use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Exit status of a process.
///
/// This is an opaque type that wraps the platform's native exit status
/// representation. Use the provided methods to query the exit status.
///
/// On Unix, the raw value is the status from `waitpid()`. On Windows, it is the exit code
/// from `GetExitCodeProcess()`.
#[derive(Eq, PartialEq, Hash, Copy, Clone)]
pub struct ExitStatus(pub(crate) Option<os::RawExitStatus>);

impl ExitStatus {
    /// Create an `ExitStatus` from the raw platform value.
    pub(crate) fn from_raw(raw: os::RawExitStatus) -> ExitStatus {
        ExitStatus(Some(raw))
    }

    /// True if the exit status of the process is 0.
    pub fn success(&self) -> bool {
        self.code() == Some(0)
    }

    /// True if the subprocess was killed by a signal with the specified
    /// number.
    ///
    /// Always returns `false` on Windows.
    pub fn is_killed_by(&self, signum: i32) -> bool {
        self.signal() == Some(signum)
    }
}

/// A handle to a running or finished subprocess.
///
/// `Process` is a lightweight handle that tracks a child process's lifecycle.  It is
/// created internally by [`Exec::start`] and [`Pipeline::start`] and appears as part of
/// the [`Job`] struct.
///
/// Unlike `std::process::Child`, all methods on `Process` take `&self` rather than `&mut
/// self`, so a `Process` can be shared between threads without external synchronization.
///
/// `Process` is cheaply cloneable. Clones share the same underlying process handle, so
/// e.g. calling `wait()` on one clone will also make the exit status available to all
/// other clones.
///
/// # Drop behavior
///
/// When the last clone of a `Process` is dropped, it waits for the child process to
/// finish unless [`detach`](Self::detach) has been called. Because `Process` does not own
/// any pipes to the child, callers must ensure that any pipes connected to the child's
/// stdin are dropped *before* the `Process` is dropped. Otherwise, the child may block
/// waiting for input while the `Process` drop waits for the child to exit, resulting in a
/// deadlock. [`Job`] handles this automatically via field declaration order.
///
/// [`Exec::start`]: crate::Exec::start
/// [`Pipeline::start`]: crate::Pipeline::start
/// [`Job`]: crate::Job
#[derive(Clone)]
pub struct Process(Arc<InnerProcess>);

struct InnerProcess {
    pid: u32,
    #[allow(dead_code)]
    ext: os::ExtProcessState,
    state: Mutex<ProcessState>,
    detached: AtomicBool,
}

#[derive(Debug)]
enum ProcessState {
    Running,
    Finished(ExitStatus),
}

impl Process {
    pub(crate) fn new(pid: u32, ext: os::ExtProcessState, detached: bool) -> Process {
        Process(Arc::new(InnerProcess {
            pid,
            ext,
            state: Mutex::new(ProcessState::Running),
            detached: AtomicBool::new(detached),
        }))
    }

    /// Returns the PID of the subprocess.
    pub fn pid(&self) -> u32 {
        self.0.pid
    }

    /// Returns the exit status, if the process is known to have finished.
    ///
    /// This does not perform any system calls. To check whether the process has finished,
    /// use [`poll`](Self::poll) or [`wait`](Self::wait).
    pub fn exit_status(&self) -> Option<ExitStatus> {
        let state = self.0.state.lock().unwrap();
        match *state {
            ProcessState::Finished(status) => Some(status),
            ProcessState::Running => None,
        }
    }

    /// Check whether the process has finished, without blocking.
    ///
    /// Returns `Some(exit_status)` if the process has finished, `None` if it is still
    /// running.
    pub fn poll(&self) -> Option<ExitStatus> {
        self.wait_timeout(Duration::from_secs(0)).unwrap_or(None)
    }

    /// Wait for the process to finish and return its exit status.
    ///
    /// If the process has already finished, returns the cached exit status immediately.
    pub fn wait(&self) -> io::Result<ExitStatus> {
        self.0.os_wait()
    }

    /// Wait for the process to finish, timing out after the specified duration.
    ///
    /// Returns `Ok(None)` if the timeout elapsed before the process finished.
    pub fn wait_timeout(&self, dur: Duration) -> io::Result<Option<ExitStatus>> {
        self.0.os_wait_timeout(dur)
    }

    /// Terminate the subprocess.
    ///
    /// On Unix, this sends SIGTERM. On Windows, this calls `TerminateProcess`.
    pub fn terminate(&self) -> io::Result<()> {
        self.0.os_terminate()
    }

    /// Kill the subprocess.
    ///
    /// On Unix, this sends SIGKILL. On Windows, this calls `TerminateProcess`.
    pub fn kill(&self) -> io::Result<()> {
        self.0.os_kill()
    }

    /// Mark the process as detached.
    ///
    /// A detached process will not be waited on when the `Process` is dropped.
    pub fn detach(&self) {
        self.0.detached.store(true, Ordering::Relaxed);
    }
}

impl Drop for InnerProcess {
    fn drop(&mut self) {
        if !self.detached.load(Ordering::Relaxed) {
            let state = self.state.get_mut().unwrap();
            if matches!(*state, ProcessState::Running) {
                let _ = self.os_wait();
            }
        }
    }
}

impl fmt::Debug for Process {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.0.state.lock().unwrap();
        f.debug_struct("Process")
            .field("pid", &self.0.pid)
            .field("state", &*state)
            .field("detached", &self.0.detached.load(Ordering::Relaxed))
            .finish()
    }
}

#[cfg(unix)]
mod os {
    use super::*;
    use crate::posix;

    pub type ExtProcessState = ();
    pub type RawExitStatus = i32;

    impl ExitStatus {
        /// Returns the exit code if the process exited normally.
        ///
        /// On Unix, this returns `Some` only if the process exited voluntarily (not
        /// killed by a signal).
        pub fn code(&self) -> Option<u32> {
            let raw = self.0?;
            libc::WIFEXITED(raw).then(|| libc::WEXITSTATUS(raw) as u32)
        }

        /// Returns the signal number if the process was killed by a signal.
        pub fn signal(&self) -> Option<i32> {
            let raw = self.0?;
            libc::WIFSIGNALED(raw).then(|| libc::WTERMSIG(raw))
        }
    }

    impl fmt::Display for ExitStatus {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.0 {
                Some(raw) if libc::WIFEXITED(raw) => {
                    write!(f, "exit code {}", libc::WEXITSTATUS(raw))
                }
                Some(raw) if libc::WIFSIGNALED(raw) => {
                    write!(f, "signal {}", libc::WTERMSIG(raw))
                }
                Some(raw) => {
                    write!(f, "unrecognized wait status: {} {:#x}", raw, raw)
                }
                None => write!(f, "undetermined exit status"),
            }
        }
    }

    impl fmt::Debug for ExitStatus {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.0 {
                Some(raw) if libc::WIFEXITED(raw) => {
                    write!(f, "ExitStatus(Exited({}))", libc::WEXITSTATUS(raw))
                }
                Some(raw) if libc::WIFSIGNALED(raw) => {
                    write!(f, "ExitStatus(Signal({}))", libc::WTERMSIG(raw))
                }
                Some(raw) => {
                    write!(f, "ExitStatus(Unknown({} {:#x}))", raw, raw)
                }
                None => write!(f, "ExitStatus(Undetermined)"),
            }
        }
    }

    impl InnerProcess {
        pub(super) fn os_wait(&self) -> io::Result<ExitStatus> {
            let mut state = self.state.lock().unwrap();
            loop {
                match *state {
                    ProcessState::Finished(status) => return Ok(status),
                    ProcessState::Running => {
                        Self::waitpid_into(&mut state, self.pid, true)?;
                    }
                }
            }
        }

        pub(super) fn os_wait_timeout(&self, dur: Duration) -> io::Result<Option<ExitStatus>> {
            use std::cmp::min;
            use std::time::Instant;

            let mut state = self.state.lock().unwrap();
            if let ProcessState::Finished(status) = *state {
                return Ok(Some(status));
            }

            let deadline = Instant::now() + dur;
            let mut delay = Duration::from_millis(1);

            loop {
                Self::waitpid_into(&mut state, self.pid, false)?;
                if let ProcessState::Finished(status) = *state {
                    return Ok(Some(status));
                }
                let now = Instant::now();
                if now >= deadline {
                    return Ok(None);
                }
                let remaining = deadline.duration_since(now);
                // Release the lock while sleeping so other threads can access the state.
                drop(state);
                std::thread::sleep(min(delay, remaining));
                delay = min(delay * 2, Duration::from_millis(100));
                state = self.state.lock().unwrap();
                // Re-check after re-acquiring lock
                if let ProcessState::Finished(status) = *state {
                    return Ok(Some(status));
                }
            }
        }

        pub(super) fn os_terminate(&self) -> io::Result<()> {
            self.send_signal(posix::SIGTERM)
        }

        pub(super) fn os_kill(&self) -> io::Result<()> {
            self.send_signal(posix::SIGKILL)
        }

        fn send_signal(&self, signal: i32) -> io::Result<()> {
            let state = self.state.lock().unwrap();
            match *state {
                ProcessState::Finished(_) => Ok(()),
                ProcessState::Running => posix::kill(self.pid, signal),
            }
        }

        fn send_signal_group(&self, signal: i32) -> io::Result<()> {
            let state = self.state.lock().unwrap();
            match *state {
                ProcessState::Finished(_) => Ok(()),
                ProcessState::Running => posix::killpg(self.pid, signal),
            }
        }

        fn waitpid_into(state: &mut ProcessState, pid: u32, block: bool) -> io::Result<()> {
            if matches!(*state, ProcessState::Finished(_)) {
                return Ok(());
            }
            match posix::waitpid(pid, if block { 0 } else { posix::WNOHANG }) {
                Ok((pid_out, exit_status)) if pid_out == pid => {
                    *state = ProcessState::Finished(exit_status);
                }
                Ok(_) => {}
                Err(e) if e.raw_os_error() == Some(posix::ECHILD) => {
                    // Someone else waited for the child. The PID no longer exists and we
                    // cannot find its exit status.
                    *state = ProcessState::Finished(ExitStatus(None));
                }
                Err(e) => return Err(e),
            }
            Ok(())
        }
    }

    pub mod ext {
        use super::*;

        /// Unix-specific extension methods for [`Process`].
        pub trait ProcessExt {
            /// Send the specified signal to the child process.
            ///
            /// If the child process is known to have finished (due to e.g.  a previous
            /// call to [`wait`] or [`poll`]), this will do nothing and return `Ok`.
            ///
            /// [`poll`]: crate::Process::poll
            /// [`wait`]: crate::Process::wait
            fn send_signal(&self, signal: i32) -> io::Result<()>;

            /// Send the specified signal to the child's process group.
            ///
            /// This is useful for terminating a tree of processes spawned by the
            /// child. For this to work correctly, the child should be started with
            /// [`ExecExt::setpgid`] set, which places the child in a new process group
            /// with PGID equal to its PID.
            ///
            /// If the child process is known to have finished, this will do nothing and
            /// return `Ok`.
            ///
            /// [`ExecExt::setpgid`]: crate::ExecExt::setpgid
            fn send_signal_group(&self, signal: i32) -> io::Result<()>;
        }

        impl ProcessExt for Process {
            fn send_signal(&self, signal: i32) -> io::Result<()> {
                self.0.send_signal(signal)
            }

            fn send_signal_group(&self, signal: i32) -> io::Result<()> {
                self.0.send_signal_group(signal)
            }
        }
    }
}

#[cfg(windows)]
mod os {
    use super::*;
    use crate::win32;
    use std::time::Duration;

    #[derive(Debug)]
    pub struct ExtProcessState(pub(crate) win32::Handle);

    pub type RawExitStatus = u32;

    impl ExitStatus {
        /// Returns the exit code if the process exited normally.
        ///
        /// On Windows, this always returns `Some` for a determined exit
        /// status.
        pub fn code(&self) -> Option<u32> {
            self.0
        }

        /// Returns the signal number if the process was killed by a signal.
        ///
        /// On Windows, this always returns `None`.
        pub fn signal(&self) -> Option<i32> {
            None
        }
    }

    impl fmt::Display for ExitStatus {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.0 {
                Some(code) => write!(f, "exit code {}", code),
                None => write!(f, "undetermined exit status"),
            }
        }
    }

    impl fmt::Debug for ExitStatus {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.0 {
                Some(code) => {
                    write!(f, "ExitStatus(Exited({}))", code)
                }
                None => write!(f, "ExitStatus(Undetermined)"),
            }
        }
    }

    impl InnerProcess {
        pub(super) fn os_wait(&self) -> io::Result<ExitStatus> {
            {
                let state = self.state.lock().unwrap();
                if let ProcessState::Finished(status) = *state {
                    return Ok(status);
                }
            }
            // Wait without holding the lock - the handle is immutable and
            // doesn't need mutex protection.
            let event = win32::WaitForSingleObject(&self.ext.0, None)?;
            let mut state = self.state.lock().unwrap();
            if let ProcessState::Finished(status) = *state {
                return Ok(status);
            }
            if let win32::WaitEvent::OBJECT_0 = event {
                let exit_code = win32::GetExitCodeProcess(&self.ext.0)?;
                *state = ProcessState::Finished(ExitStatus::from_raw(exit_code));
                Ok(ExitStatus::from_raw(exit_code))
            } else {
                Err(io::Error::other(
                    "os_wait: child state is not Finished after WaitForSingleObject",
                ))
            }
        }

        pub(super) fn os_wait_timeout(&self, dur: Duration) -> io::Result<Option<ExitStatus>> {
            {
                let state = self.state.lock().unwrap();
                if let ProcessState::Finished(status) = *state {
                    return Ok(Some(status));
                }
            }
            // Wait without holding the lock - the handle is immutable and
            // doesn't need mutex protection.
            let event = win32::WaitForSingleObject(&self.ext.0, Some(dur))?;
            let mut state = self.state.lock().unwrap();
            if let ProcessState::Finished(status) = *state {
                return Ok(Some(status));
            }
            if let win32::WaitEvent::OBJECT_0 = event {
                let exit_code = win32::GetExitCodeProcess(&self.ext.0)?;
                *state = ProcessState::Finished(ExitStatus::from_raw(exit_code));
                Ok(Some(ExitStatus::from_raw(exit_code)))
            } else {
                Ok(None)
            }
        }

        pub(super) fn os_terminate(&self) -> io::Result<()> {
            let mut state = self.state.lock().unwrap();
            if let ProcessState::Running = *state
                && let Err(err) = win32::TerminateProcess(&self.ext.0, 1)
            {
                if err.raw_os_error() != Some(win32::ERROR_ACCESS_DENIED as i32) {
                    return Err(err);
                }
                let rc = win32::GetExitCodeProcess(&self.ext.0)?;
                if rc == win32::STILL_ACTIVE {
                    return Err(err);
                }
                *state = ProcessState::Finished(ExitStatus::from_raw(rc));
            }
            Ok(())
        }

        pub(super) fn os_kill(&self) -> io::Result<()> {
            self.os_terminate()
        }
    }

    pub mod ext {}
}

#[cfg(windows)]
pub(crate) use os::ExtProcessState;
#[cfg(unix)]
pub use os::ext::*;
