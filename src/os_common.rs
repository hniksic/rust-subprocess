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
    /// This should not occur in normal operation, but is possible if
    /// for example some foreign code calls `waitpid()` on the PID of
    /// the child process.
    Undetermined,
}

impl ExitStatus {
    /// True if the exit status of the process is 0.
    pub fn success(self) -> bool {
        if let ExitStatus::Exited(0) = self {
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
