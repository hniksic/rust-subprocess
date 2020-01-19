use std::cell::RefCell;
use std::fs::File;
use std::io;
use std::rc::Rc;

/// Exit status of a process.

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum ExitStatus {
    /// The process exited with the specified exit code.
    ///
    /// Note that the exit code is limited to a much smaller range on
    /// most platforms.
    Exited(u32),

    /// The process exited due to a signal with the specified number.
    ///
    /// This variant is never created on Windows, where signals of
    /// Unix kind do not exist.
    Signaled(u8),

    /// The process exit status cannot be described by the preceding
    /// two variants.
    ///
    /// This should not occur in normal operation.
    Other(i32),

    /// It is known that the process has completed, but its exit
    /// status is unavailable.
    ///
    /// This should not occur in normal operation, but if possible if
    /// for example some foreign code calls `waitpid()` on the PID of
    /// the child process.
    Undetermined,
}

impl ExitStatus {
    /// True if the exit status of the process is 0.
    pub fn success(&self) -> bool {
        if let &ExitStatus::Exited(0) = self {
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Copy, Clone)]
#[allow(dead_code)]
pub enum StandardStream {
    Input = 0,
    Output = 1,
    Error = 2,
}

thread_local! {
    static STREAMS: RefCell<[Option<Rc<File>>; 3]> = RefCell::default();
}

#[cfg(unix)]
use crate::posix::make_standard_stream;
#[cfg(windows)]
use crate::win32::make_standard_stream;

pub fn get_standard_stream(which: StandardStream) -> io::Result<Rc<File>> {
    STREAMS.with(|streams| {
        if let Some(ref stream) = streams.borrow()[which as usize] {
            return Ok(stream.clone());
        }
        let stream = make_standard_stream(which)?;
        streams.borrow_mut()[which as usize] = Some(stream.clone());
        Ok(stream.clone())
    })
}
