#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum ExitStatus {
    Exited(u32),
    Signaled(u8),
    Other(i32),
}

pub enum StandardStream {
    Input,
    Output,
    Error,
}
