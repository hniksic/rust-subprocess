//! Execution of and interaction with external processes and pipelines.
//!
//! The entry points to the crate are the [`Popen`] struct and the [`Exec`] builder.  `Popen`
//! is the interface to a running child process, inspired by Python's [`subprocess.Popen`].
//! `Exec` provides a builder-pattern API with convenient methods for streaming and capturing
//! of output, as well as combining `Popen` instances into pipelines.
//!
//! Compared to `std::process`, the crate provides these additional features:
//!
//! * The *communicate* [family of methods](Popen::communicate) for deadlock-free
//!   capturing of subprocess output/error, while simultaneously feeding data to its standard
//!   input.  Capturing supports optional timeout and read size limit.
//!
//! * Connecting multiple commands into OS-level [pipelines](Pipeline).
//!
//! * Flexible [redirection options](Redirection), such as connecting standard streams to
//!   arbitrary [open files](Redirection::File), or [merging](Redirection::Merge) output
//!   streams like shell's `2>&1` and `1>&2` operators.
//!
//! * Non-blocking and timeout methods to wait on the process: [`poll`](Popen::poll),
//!   [`wait`](Popen::wait), and [`wait_timeout`](Popen::wait_timeout).
//!
//! # Examples
//!
//! Communicate with a process and optionally terminate it:
//!
//! ```
//! # use subprocess::*;
//! # fn dummy() -> Result<()> {
//! let mut p = Popen::create(&["ps", "x"], PopenConfig {
//!     stdout: Redirection::Pipe, ..Default::default()
//! })?;
//!
//! // Obtain the output from the standard streams.
//! let (out, err) = p.communicate([])?.read_string()?;
//!
//! if let Some(exit_status) = p.poll() {
//!     // the process has finished
//! } else {
//!     // it is still running, terminate it
//!     p.terminate()?;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Use the [`Exec`] builder to execute a pipeline of commands and capture the output:
//!
//! ```no_run
//! # use subprocess::*;
//! # fn dummy() -> Result<()> {
//! let dir_checksum = {
//!     Exec::shell("find . -type f") | Exec::cmd("sort") | Exec::cmd("sha1sum")
//! }.capture()?.stdout_str();
//! # Ok(())
//! # }
//! ```
//!
//! [`subprocess.Popen`]: https://docs.python.org/3/library/subprocess.html#subprocess.Popen

#![warn(missing_debug_implementations, missing_docs)]
#![allow(clippy::type_complexity)]

mod builder;
mod communicate;
mod popen;

#[cfg(unix)]
mod posix;

#[cfg(windows)]
mod win32;

mod os_common;

#[cfg(test)]
mod tests;

pub use builder::{Capture, Exec, ExecExt, Pipeline};
pub use communicate::Communicator;
pub use os_common::ExitStatus;
pub use popen::{_PrivateSeal, Popen, PopenConfig, Redirection, Result, make_pipe};

/// Subprocess extensions for Unix platforms.
#[cfg(unix)]
pub mod unix {
    pub use super::popen::os_ext::*;
}

/// Subprocess extensions for Windows platforms.
#[cfg(windows)]
pub mod windows {
    pub use super::builder::windows::*;
}
