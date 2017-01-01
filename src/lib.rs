extern crate libc;

#[cfg(windows)]
extern crate kernel32;
#[cfg(windows)]
extern crate winapi;

mod popen;
mod build;

#[cfg(unix)]
mod posix;

#[cfg(windows)]
mod win32;

mod common;

pub use self::common::ExitStatus;
pub use self::popen::{Popen, PopenConfig, Redirection, PopenError};
pub use self::build::{Run, NullFile, Pipeline};


#[cfg(test)]
mod tests {
    mod common;
    #[cfg(unix)]
    mod posix;
    #[cfg(windows)]
    mod win32;
    mod build;
}
