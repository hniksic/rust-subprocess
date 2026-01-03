#![allow(non_snake_case, non_camel_case_types)]

use std::cell::UnsafeCell;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Error, Result};
use std::iter;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use winapi::shared::{
    minwindef::{BOOL, DWORD, FALSE, LPVOID, TRUE},
    winerror::{
        ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF, ERROR_IO_PENDING, ERROR_NOT_FOUND, WAIT_TIMEOUT,
    },
};
use winapi::um::fileapi::CreateFileW;
use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
use winapi::um::ioapiset::GetOverlappedResult;
use winapi::um::minwinbase::{LPSECURITY_ATTRIBUTES, OVERLAPPED, SECURITY_ATTRIBUTES};
use winapi::um::namedpipeapi::CreateNamedPipeW;
use winapi::um::processthreadsapi::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW};
use winapi::um::synchapi::CreateEventW;
use winapi::um::winbase::{
    CREATE_UNICODE_ENVIRONMENT, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED,
    PIPE_ACCESS_OUTBOUND, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use winapi::um::winbase::{INFINITE, WAIT_ABANDONED, WAIT_ABANDONED_0, WAIT_FAILED, WAIT_OBJECT_0};
use winapi::um::winnt::GENERIC_READ;
use winapi::um::{fileapi, handleapi, processenv, processthreadsapi, synchapi};

pub use winapi::shared::winerror::{ERROR_ACCESS_DENIED, ERROR_BAD_PATHNAME};
pub const STILL_ACTIVE: u32 = 259;

use crate::os_common::StandardStream;

#[derive(Debug)]
pub struct Handle(RawHandle);

unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.as_raw_handle());
        }
    }
}

impl AsRawHandle for Handle {
    fn as_raw_handle(&self) -> RawHandle {
        self.0
    }
}

impl FromRawHandle for Handle {
    unsafe fn from_raw_handle(handle: RawHandle) -> Handle {
        Handle(handle)
    }
}

pub const HANDLE_FLAG_INHERIT: u32 = 1;
pub const STARTF_USESTDHANDLES: DWORD = winapi::um::winbase::STARTF_USESTDHANDLES;

fn check(status: BOOL) -> Result<()> {
    if status != 0 {
        Ok(())
    } else {
        Err(Error::last_os_error())
    }
}

fn check_handle(raw_handle: RawHandle) -> Result<RawHandle> {
    if raw_handle != INVALID_HANDLE_VALUE {
        Ok(raw_handle)
    } else {
        Err(Error::last_os_error())
    }
}

// OsStr to zero-terminated owned vector
fn to_nullterm(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(iter::once(0u16)).collect()
}

// Counter for generating unique pipe names
static PIPE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_pipe_name() -> Vec<u16> {
    let pid = std::process::id();
    let counter = PIPE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!(r"\\.\pipe\subprocess_{}_{}", pid, counter);
    name.encode_utf16().chain(iter::once(0u16)).collect()
}

/// Create a pipe where both ends support overlapped I/O.
///
/// Returns (read_handle, write_handle), both with FILE_FLAG_OVERLAPPED.
/// Both handles are inheritable; use SetHandleInformation to change this.
pub fn CreateOverlappedPipe() -> Result<(File, File)> {
    let pipe_name = unique_pipe_name();
    const BUFFER_SIZE: DWORD = 4096;

    let mut sa = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as DWORD,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: TRUE,
    };

    // Create the write end as server (named pipe), then connect read end as client.
    // Both ends get FILE_FLAG_OVERLAPPED.
    let write_handle = check_handle(unsafe {
        CreateNamedPipeW(
            pipe_name.as_ptr(),
            PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            BUFFER_SIZE,
            BUFFER_SIZE,
            0,
            &mut sa as LPSECURITY_ATTRIBUTES,
        )
    })?;
    let read_handle = check_handle(unsafe {
        CreateFileW(
            pipe_name.as_ptr(),
            GENERIC_READ,
            0,
            &mut sa as LPSECURITY_ATTRIBUTES,
            fileapi::OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            ptr::null_mut(),
        )
    })?;
    Ok(unsafe {
        (
            File::from_raw_handle(read_handle),
            File::from_raw_handle(write_handle),
        )
    })
}

/// Create a manual-reset event object for use with overlapped I/O.
fn CreateEvent() -> Result<Handle> {
    let handle = unsafe { CreateEventW(ptr::null_mut(), TRUE, FALSE, ptr::null()) };
    check_handle(handle)?;
    Ok(unsafe { Handle::from_raw_handle(handle) })
}

/// Reset an event to non-signaled state.
fn ResetEvent(event: &Handle) -> Result<()> {
    check(unsafe { synchapi::ResetEvent(event.as_raw_handle()) })
}

/// Get the result of an overlapped operation.
/// Returns Ok(bytes_transferred) or Err if the operation failed.
/// ERROR_BROKEN_PIPE and ERROR_HANDLE_EOF are treated as EOF (returns 0 bytes).
fn get_overlapped_result(
    handle: RawHandle,
    overlapped: &mut OVERLAPPED,
    wait: bool,
) -> Result<u32> {
    let mut bytes_transferred: DWORD = 0;
    let result =
        unsafe { GetOverlappedResult(handle, overlapped, &mut bytes_transferred, wait as BOOL) };
    if result != 0 {
        Ok(bytes_transferred)
    } else {
        let err = Error::last_os_error();
        let code = err.raw_os_error();
        if code == Some(ERROR_BROKEN_PIPE as i32) || code == Some(ERROR_HANDLE_EOF as i32) {
            // Pipe closed or EOF
            Ok(0)
        } else {
            Err(err)
        }
    }
}

/// State of a pending overlapped operation.
#[derive(Debug, Clone, Copy)]
enum PendingState {
    /// Operation is pending.
    Pending,
    /// Operation completed with this many bytes transferred.
    Completed(u32),
}

/// A pending overlapped read operation.
///
/// This type owns the buffer being read into and will cancel the I/O operation
/// on drop if it hasn't completed. Use `is_pending()` to check status, `event()`
/// to get a handle for `WaitForMultipleObjects`, and `complete()` to finish the
/// operation and retrieve the byte count.
pub struct PendingRead {
    handle: RawHandle,
    overlapped: Box<OVERLAPPED>,
    event: Handle,
    /// Buffer wrapped in UnsafeCell because the OS writes to it asynchronously
    /// while we may hold shared references to the struct.
    buffer: UnsafeCell<Box<[u8]>>,
    state: PendingState,
}

impl std::fmt::Debug for PendingRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingRead")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl PendingRead {
    /// Returns true if the operation is ready.
    pub fn is_ready(&self) -> bool {
        matches!(self.state, PendingState::Completed(_))
    }

    /// Get the event handle for use with `WaitForMultipleObjects`.
    pub fn event(&self) -> &Handle {
        &self.event
    }

    /// Complete the operation and return the number of bytes read.
    ///
    /// If already completed, returns the cached result. If pending, retrieves
    /// the result (which should only be called after the event is signaled).
    pub fn complete(&mut self) -> Result<u32> {
        match self.state {
            PendingState::Completed(n) => Ok(n),
            PendingState::Pending => {
                let n = get_overlapped_result(self.handle, &mut self.overlapped, false)?;
                self.state = PendingState::Completed(n);
                Ok(n)
            }
        }
    }

    /// Get the data from a completed read.
    ///
    /// Panics if called before `complete()`.
    pub fn data(&self) -> &[u8] {
        match self.state {
            PendingState::Completed(n) => {
                // SAFETY: We only access the buffer after the operation has completed,
                // so the OS is no longer writing to it.
                let buffer = unsafe { &*self.buffer.get() };
                &buffer[..n as usize]
            }
            PendingState::Pending => panic!("called data() on pending read"),
        }
    }
}

impl Drop for PendingRead {
    fn drop(&mut self) {
        if !self.is_ready() {
            let _ = CancelIoEx(self.handle, &mut self.overlapped);
            let _ = get_overlapped_result(self.handle, &mut self.overlapped, true);
        }
    }
}

/// A pending overlapped write operation.
///
/// This type owns a copy of the data being written and will cancel the I/O
/// operation on drop if it hasn't completed. Use `is_ready()` to check status,
/// `event()` to get a handle for `WaitForMultipleObjects`, and `complete()` to
/// finish the operation and retrieve the byte count.
pub struct PendingWrite {
    handle: RawHandle,
    overlapped: Box<OVERLAPPED>,
    event: Handle,
    buffer: Box<[u8]>,
    state: PendingState,
}

impl std::fmt::Debug for PendingWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingWrite")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl PendingWrite {
    /// Returns true if the operation is still pending.
    pub fn is_ready(&self) -> bool {
        matches!(self.state, PendingState::Completed(_))
    }

    /// Get the event handle for use with `WaitForMultipleObjects`.
    pub fn event(&self) -> &Handle {
        &self.event
    }

    /// Complete the operation and return the number of bytes written.
    ///
    /// If already completed, returns the cached result. If pending, retrieves
    /// the result (which should only be called after the event is signaled).
    pub fn complete(&mut self) -> Result<u32> {
        match self.state {
            PendingState::Completed(n) => Ok(n),
            PendingState::Pending => {
                let n = get_overlapped_result(self.handle, &mut self.overlapped, false)?;
                self.state = PendingState::Completed(n);
                Ok(n)
            }
        }
    }
}

impl Drop for PendingWrite {
    fn drop(&mut self) {
        if !self.is_ready() {
            let _ = CancelIoEx(self.handle, &mut self.overlapped);
            let _ = get_overlapped_result(self.handle, &mut self.overlapped, true);
        }
    }
}

/// Start an overlapped read operation.
pub fn ReadFileOverlapped(handle: RawHandle, buffer_size: usize) -> Result<PendingRead> {
    let event = CreateEvent()?;
    let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { mem::zeroed() });
    overlapped.hEvent = event.as_raw_handle();

    let buffer: Box<[u8]> = vec![0u8; buffer_size].into_boxed_slice();
    let mut pending = PendingRead {
        handle,
        overlapped,
        event,
        buffer: UnsafeCell::new(buffer),
        state: PendingState::Pending,
    };

    ResetEvent(&pending.event)?;
    let mut bytes_read: DWORD = 0;
    // SAFETY: We pass a pointer to the buffer which we own. The OS will write to it
    // asynchronously, which is why the buffer is wrapped in UnsafeCell.
    let result = unsafe {
        let buffer = &mut *pending.buffer.get();
        fileapi::ReadFile(
            handle,
            buffer.as_mut_ptr() as LPVOID,
            buffer.len() as DWORD,
            &mut bytes_read,
            pending.overlapped.as_mut() as _,
        )
    };
    if result != 0 {
        pending.state = PendingState::Completed(bytes_read);
    } else {
        let err = Error::last_os_error();
        let code = err.raw_os_error();
        if code == Some(ERROR_IO_PENDING as i32) {
            // Already set to Pending
        } else if code == Some(ERROR_BROKEN_PIPE as i32) || code == Some(ERROR_HANDLE_EOF as i32) {
            pending.state = PendingState::Completed(0);
        } else {
            return Err(err);
        }
    }
    Ok(pending)
}

/// Start an overlapped write operation.
pub fn WriteFileOverlapped(handle: RawHandle, data: &[u8]) -> Result<PendingWrite> {
    let event = CreateEvent()?;
    let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { mem::zeroed() });
    overlapped.hEvent = event.as_raw_handle();

    let mut pending = PendingWrite {
        handle,
        overlapped,
        event,
        buffer: data.into(),
        state: PendingState::Pending,
    };

    ResetEvent(&pending.event)?;
    let mut bytes_written: DWORD = 0;
    let result = unsafe {
        fileapi::WriteFile(
            handle,
            pending.buffer.as_ptr() as LPVOID,
            pending.buffer.len() as DWORD,
            &mut bytes_written,
            pending.overlapped.as_mut() as _,
        )
    };
    if result != 0 {
        pending.state = PendingState::Completed(bytes_written);
    } else {
        let err = Error::last_os_error();
        if err.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
            // Already set to Pending
        } else {
            return Err(err);
        }
    }
    Ok(pending)
}

pub enum WaitResult {
    Object(usize), // Index of the signaled object
    Timeout,
}

/// Wait for multiple objects, returns the index of the first signaled object.
pub fn WaitForMultipleObjects(
    handles: &[RawHandle],
    timeout: Option<Duration>,
) -> Result<WaitResult> {
    assert!(
        handles.len() <= 64,
        "WaitForMultipleObjects: max 64 handles"
    );

    let mut remaining_timeout = timeout;
    let deadline = timeout.map(|t| Instant::now() + t);

    loop {
        let (timeout_ms, overflow) = remaining_timeout
            .map(|timeout| {
                let timeout = timeout.as_millis();
                if timeout < INFINITE as u128 {
                    (timeout as u32, false)
                } else {
                    (INFINITE - 1, true)
                }
            })
            .unwrap_or((INFINITE, false));

        let result = unsafe {
            synchapi::WaitForMultipleObjects(
                handles.len() as DWORD,
                handles.as_ptr(),
                FALSE, // wait for any, not all
                timeout_ms,
            )
        };

        if result < WAIT_OBJECT_0 + handles.len() as u32 {
            return Ok(WaitResult::Object((result - WAIT_OBJECT_0) as usize));
        } else if result >= WAIT_ABANDONED_0 && result < WAIT_ABANDONED_0 + handles.len() as u32 {
            // Treat abandoned mutex like signaled
            return Ok(WaitResult::Object((result - WAIT_ABANDONED_0) as usize));
        } else if result == WAIT_TIMEOUT {
            if !overflow {
                return Ok(WaitResult::Timeout);
            }
            // Timeout overflowed, check if we're past the deadline
            let deadline = deadline.unwrap();
            let now = Instant::now();
            if now >= deadline {
                return Ok(WaitResult::Timeout);
            }
            remaining_timeout = Some(deadline - now);
            continue;
        } else if result == WAIT_FAILED {
            return Err(Error::last_os_error());
        } else {
            panic!(
                "WaitForMultipleObjects returned unexpected value {}",
                result
            );
        }
    }
}

pub fn SetHandleInformation(handle: &File, dwMask: u32, dwFlags: u32) -> Result<()> {
    check(unsafe { handleapi::SetHandleInformation(handle.as_raw_handle(), dwMask, dwFlags) })?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn CreateProcess(
    appname: Option<&OsStr>,
    cmdline: &OsStr,
    env_block: &Option<Vec<u16>>,
    cwd: &Option<&OsStr>,
    inherit_handles: bool,
    mut creation_flags: u32,
    stdin: Option<RawHandle>,
    stdout: Option<RawHandle>,
    stderr: Option<RawHandle>,
    sinfo_flags: u32,
) -> Result<(Handle, u64)> {
    let mut sinfo: STARTUPINFOW = unsafe { mem::zeroed() };
    sinfo.cb = mem::size_of::<STARTUPINFOW>() as DWORD;
    sinfo.hStdInput = stdin.unwrap_or(ptr::null_mut());
    sinfo.hStdOutput = stdout.unwrap_or(ptr::null_mut());
    sinfo.hStdError = stderr.unwrap_or(ptr::null_mut());
    sinfo.dwFlags = sinfo_flags;
    let mut pinfo: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    let mut cmdline = to_nullterm(cmdline);
    let wc_appname = appname.map(to_nullterm);
    let env_block_ptr = env_block
        .as_ref()
        .map(|v| v.as_ptr())
        .unwrap_or(ptr::null()) as LPVOID;
    let cwd = cwd.map(to_nullterm);
    creation_flags |= CREATE_UNICODE_ENVIRONMENT;
    check(unsafe {
        CreateProcessW(
            wc_appname
                .as_ref()
                .map(|v| v.as_ptr())
                .unwrap_or(ptr::null()),
            cmdline.as_mut_ptr(),
            ptr::null_mut(),         // lpProcessAttributes
            ptr::null_mut(),         // lpThreadAttributes
            inherit_handles as BOOL, // bInheritHandles
            creation_flags,          // dwCreationFlags
            env_block_ptr,           // lpEnvironment
            cwd.as_ref().map(|v| v.as_ptr()).unwrap_or(ptr::null()), // lpCurrentDirectory
            &mut sinfo,
            &mut pinfo,
        )
    })?;
    unsafe {
        drop(Handle::from_raw_handle(pinfo.hThread));
        Ok((
            Handle::from_raw_handle(pinfo.hProcess),
            pinfo.dwProcessId as u64,
        ))
    }
}

#[allow(clippy::upper_case_acronyms)]
pub enum WaitEvent {
    OBJECT_0,
    ABANDONED,
    TIMEOUT,
}

pub fn WaitForSingleObject(handle: &Handle, mut timeout: Option<Duration>) -> Result<WaitEvent> {
    let deadline = timeout.map(|timeout| Instant::now() + timeout);

    let result = loop {
        // Allow timeouts greater than 50 days by clamping the
        // timeout and sleeping in a loop.
        let (timeout_ms, overflow) = timeout
            .map(|timeout| {
                let timeout = timeout.as_millis();
                if timeout < INFINITE as u128 {
                    (timeout as u32, false)
                } else {
                    (INFINITE - 1, true)
                }
            })
            .unwrap_or((INFINITE, false));

        let result = unsafe { synchapi::WaitForSingleObject(handle.as_raw_handle(), timeout_ms) };
        if result != WAIT_TIMEOUT || !overflow {
            break result;
        }
        let deadline = deadline.unwrap();
        let now = Instant::now();
        if now >= deadline {
            break WAIT_TIMEOUT;
        }
        timeout = Some(deadline - now);
    };

    if result == WAIT_OBJECT_0 {
        Ok(WaitEvent::OBJECT_0)
    } else if result == WAIT_ABANDONED {
        Ok(WaitEvent::ABANDONED)
    } else if result == WAIT_TIMEOUT {
        Ok(WaitEvent::TIMEOUT)
    } else if result == WAIT_FAILED {
        Err(Error::last_os_error())
    } else {
        panic!("WaitForSingleObject returned {}", result);
    }
}

pub fn GetExitCodeProcess(handle: &Handle) -> Result<u32> {
    let mut exit_code = 0u32;
    check(unsafe {
        processthreadsapi::GetExitCodeProcess(handle.as_raw_handle(), &mut exit_code as *mut u32)
    })?;
    Ok(exit_code)
}

pub fn TerminateProcess(handle: &Handle, exit_code: u32) -> Result<()> {
    check(unsafe { processthreadsapi::TerminateProcess(handle.as_raw_handle(), exit_code) })
}

unsafe fn GetStdHandle(which: StandardStream) -> Result<RawHandle> {
    // private/unsafe because the raw handle it returns must be
    // duplicated or leaked before converting to an owned Handle.
    use winapi::um::winbase::{STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    let id = match which {
        StandardStream::Input => STD_INPUT_HANDLE,
        StandardStream::Output => STD_OUTPUT_HANDLE,
        StandardStream::Error => STD_ERROR_HANDLE,
    };
    let raw_handle = check_handle(unsafe { processenv::GetStdHandle(id) })?;
    Ok(raw_handle)
}

pub fn make_standard_stream(which: StandardStream) -> Result<Rc<File>> {
    unsafe {
        let raw = GetStdHandle(which)?;
        let stream = Rc::new(File::from_raw_handle(raw));
        // Leak the Rc so the object we return doesn't close the underlying
        // system handle.
        mem::forget(Rc::clone(&stream));
        Ok(stream)
    }
}

/// Cancel pending overlapped I/O on a handle.
///
/// After calling this, you should call `get_overlapped_result(handle, overlapped, true)` to wait
/// for the cancellation to complete before freeing the overlapped structure or buffer.
fn CancelIoEx(handle: RawHandle, overlapped: &mut OVERLAPPED) -> Result<()> {
    let result = unsafe { winapi::um::ioapiset::CancelIoEx(handle, overlapped as _) };
    if result != 0 {
        Ok(())
    } else {
        let err = Error::last_os_error();
        // ERROR_NOT_FOUND means no pending I/O on this handle/overlapped - that's fine
        if err.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
            Ok(())
        } else {
            Err(err)
        }
    }
}
