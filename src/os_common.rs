use std::mem;

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
    /// True if the exit status is of the process is 0.
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
    Input,
    Output,
    Error,
}

// Undropped: don't drop an object after going out of scope.  This is
// used for Files made from standard descriptors.  For example:
//
// let unowned_stdin = unsafe { Undropped::new(File::from_raw_fd(0)) };
//
// This allows the use of &File corresponding to standard input
// without closing fd 0 when unowned_stdin goes out of scope.  Using
// this class is inherently dangerous, but it is useful to represent
// the system streams returned by get_standard_stream.

#[derive(Debug)]
pub struct Undropped<T>(Option<T>);

impl<T> Undropped<T> {
    pub unsafe fn new(o: T) -> Undropped<T> {
        Undropped(Some(o))
    }

    pub fn get_ref(&self) -> &T {
        self.0.as_ref().unwrap()
    }
}

impl<T> Drop for Undropped<T> {
    fn drop(&mut self) {
        let o = self.0.take().unwrap();
        mem::forget(o);
    }
}
