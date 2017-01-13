use std::mem;

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
