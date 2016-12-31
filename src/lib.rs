extern crate libc;

#[cfg(windows)]
extern crate kernel32;
#[cfg(windows)]
extern crate winapi;

mod popen;
mod run;

#[cfg(unix)]
mod posix;

#[cfg(windows)]
mod win32;

mod common;

pub use self::common::ExitStatus;
pub use self::popen::{Popen, PopenConfig, Redirection, PopenError};
pub use self::run::{Run, NullFile};


#[cfg(test)]
mod tests {
    mod common;
    #[cfg(unix)]
    mod posix;
    #[cfg(windows)]
    mod win32;
    mod run;
}
