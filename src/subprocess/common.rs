#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum ExitStatus {
    Exited(u8),
    Signaled(u8),
    Other(i32),
}

