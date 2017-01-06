extern crate libc;

#[cfg(windows)]
extern crate kernel32;
#[cfg(windows)]
extern crate winapi;

mod popen;
mod builder;

#[cfg(unix)]
mod posix;

#[cfg(windows)]
mod win32;

mod os_common;

pub use self::os_common::ExitStatus;
pub use self::popen::{Popen, PopenConfig, Redirection, PopenError};
pub use self::builder::{Exec, NullFile, Pipeline};


#[cfg(test)]
mod tests {
    mod common;
    #[cfg(unix)]
    mod posix;
    #[cfg(windows)]
    mod win32;
    mod builder;
}
