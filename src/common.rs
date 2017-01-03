#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum ExitStatus {
    Exited(u32),                // exited
    Signaled(u8),               // terminated by signal
    Other(i32),                 // other possibilities - see waitpid(2)

    // the process is known to have completed, but its exit status is
    // unavailable
    Undetermined,
}

#[derive(Debug, Copy, Clone)]
#[allow(dead_code)]
pub enum StandardStream {
    Input,
    Output,
    Error,
}
