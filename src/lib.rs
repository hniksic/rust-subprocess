extern crate libc;

#[cfg(windows)]
extern crate kernel32;
#[cfg(windows)]
extern crate winapi;

pub mod popen;

#[cfg(unix)]
mod posix;

#[cfg(windows)]
mod win32;

mod common;

pub use self::common::ExitStatus;
pub use self::popen::{Popen, Redirection};


#[cfg(test)]
mod tests {
    mod common;
    #[cfg(unix)]
    mod posix;
    #[cfg(windows)]
    mod win32;
}
