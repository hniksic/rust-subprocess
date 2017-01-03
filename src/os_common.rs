#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum ExitStatus {
    Exited(u32),                // exited
    Signaled(u8),               // terminated by signal
    Other(i32),                 // other possibilities - see waitpid(2)

    // the process is known to have completed, but its exit status is
    // unavailable
    Undetermined,
}

impl ExitStatus {
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
