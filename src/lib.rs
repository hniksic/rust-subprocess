//! Execution and interaction with external processes.
//!
//! The entry point to the module is the `Popen` struct and the `Exec`
//! builder class.  `Popen` is modeled after Python's
//! `subprocess.Popen`, with modifications to make it fit to Rust,
//! while `Exec` provides a nice Rustic builder-style API with
//! convenient methods for streaming and capturing of output, as well
//! as combining `Popen` instances into pipelines.
//!
//! Compared to `std::process`, the module follows the following
//! additional features:
//!
//! * The `communicate` method for deadlock-free reading of subprocess
//!   output/error, while simultaneously providing it stdin.
//!
//! * Advanced redirection options, such as connecting standard streams to
//!   arbitary files, or merging errors into output like shell's `2>&1`
//!   operator.
//!
//! * Non-blocking and timeout methods to wait on the process: `poll`,
//!   `wait`, and `wait_timeout`.
//!
//! * Connecting multiple commands into OS-level pipelines.
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
//! ```rust
//! let dir_checksum = {
//!     Exec::cmd("find . -type f") | Exec::cmd("sort") | Exec::cmd("sha1sum")
//! }.capture()?.output_str();
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
