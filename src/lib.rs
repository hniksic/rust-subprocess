//! Execution and interaction with external processes.
//!
//! The module has two entry points.  One is the [`Popen`] struct,
//! inspired by Python's `subprocess.Popen`.  This is useful when a
//! greater amount of control is needed, or when porting Python code
//! written for Python's `subprocess`.  The other entry point is the
//! [`Exec`] struct written in the builder style more native to Rust,
//! similar to `std::process::Command`.
//!
//! # Examples
//!
//! Create [`Popen`] directly in order to communicate with a process and
//! optionally terminate it:
//!
//! ```ignore
//! let mut p = Popen::create(&["ps", "x"], PopenConfig {
//!     stdout: Redirection::Pipe, ..Default::default()
//! })?;
//!
//! // Since we requested stdout to be redirected to a pipe, the parent's
//! // end of the pipe is available as p.stdout.  It can either be read
//! // directly, or processed using the communicate() method:
//! let (out, err) = p.communicate(None)?;
//!
//! // check if the process is still alive
//! if let Some(exit_status) = p.poll() {
//!     // the process has finished
//! } else {
//!     // it is still running, terminate it
//!     p.terminate()?;
//! }
//! ```
//!
//! Use the [`Exec`] builder to execute a command and capture its
//! output:
//!
//! ```ignore
//! let output = Exec::cmd("command").arg("arg1").arg("arg2")
//!     .stdout(Redirection::Pipe)
//!     .capture()?
//!     .stdout_str();
//! ```
//!
//! [`Popen`]: struct.Popen.html
//! [`Exec`]: struct.Exec.html

#![warn(missing_docs)]

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
pub use self::popen::{Popen, PopenConfig, Redirection, PopenError, Result};
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
