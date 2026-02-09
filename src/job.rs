use std::fs::File;
use std::io;
use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use crate::communicate::Communicator;
use crate::exec::{Capture, InputData};
use crate::process::{ExitStatus, Process};

/// Interface to a started process or pipeline.
///
/// Created by [`Exec::start`] or [`Pipeline::start`].
///
/// When dropped, waits for all processes to finish unless [`detached`](Self::detach).
#[derive(Debug)]
#[non_exhaustive]
pub struct Job {
    // Pipe fields are declared before `processes` so that they are dropped
    // first, allowing children to receive EOF and exit before
    // `Process::drop` waits on them.
    /// Write end of the first process's stdin pipe, if stdin was `Pipe`.
    pub stdin: Option<File>,
    /// Read end of the last process's stdout pipe, if stdout was `Pipe`.
    pub stdout: Option<File>,
    /// Read end of the shared stderr pipe, if stderr was `Pipe`.
    pub stderr: Option<File>,
    /// Data to feed to the first process's stdin, set by [`Exec::stdin`]
    /// or [`Pipeline::stdin`].
    pub stdin_data: InputData,
    /// Whether to return an error on non-zero exit status.
    pub check_success: bool,
    /// Started processes, in pipeline order.
    pub processes: Vec<Process>,
}

impl Job {
    /// Creates a [`Communicator`] from the pipe ends.
    ///
    /// The communicator takes ownership of `stdin`, `stdout`, and `stderr`, leaving them
    /// as `None`. Only streams that were redirected to a pipe will be available to the
    /// communicator.
    pub fn communicate(&mut self) -> io::Result<Communicator> {
        Communicator::new(
            self.stdin.take(),
            self.stdout.take(),
            self.stderr.take(),
            std::mem::take(&mut self.stdin_data),
        )
    }

    /// Terminates all processes in the pipeline.
    ///
    /// Delegates to [`Process::terminate()`] on each process, which sends `SIGTERM` on
    /// Unix and calls `TerminateProcess` on Windows. Already reaped processes are
    /// silently skipped.
    pub fn terminate(&self) -> io::Result<()> {
        for p in &self.processes {
            p.terminate()?;
        }
        Ok(())
    }

    /// Waits for all processes to finish and returns the last process's exit status.
    ///
    /// If no processes have been started (empty pipeline), returns a successful exit
    /// status.
    ///
    /// Unlike [`join`](Self::join), this does not consume `self`, does not close the pipe
    /// ends, and ignores `check_success`.
    pub fn wait(&self) -> io::Result<ExitStatus> {
        let mut status = ExitStatus::from_raw(0);
        for p in &self.processes {
            status = p.wait()?;
        }
        Ok(status)
    }

    /// Returns the PID of the last process in the pipeline.
    ///
    /// For a single command started with [`Exec::start`], this is the PID of that
    /// command. For a pipeline, this is the PID of the last command.
    ///
    /// # Panics
    ///
    /// Panics if no processes have been started because this was created by an empty
    /// `Pipeline`.
    pub fn pid(&self) -> u32 {
        self.processes.last().unwrap().pid()
    }

    /// Returns the PIDs of all processes in the pipeline, in pipeline order.
    ///
    /// If the job was started by a single process, this will return its pid. It will be
    /// empty for a job started by an empty pipeline.
    pub fn pids(&self) -> Vec<u32> {
        self.processes.iter().map(|p| p.pid()).collect()
    }

    /// Kill all processes in the pipeline.
    ///
    /// Delegates to [`Process::kill()`] on each process, which sends `SIGKILL` on Unix
    /// and calls `TerminateProcess` on Windows. Already reaped processes are silently
    /// skipped.
    pub fn kill(&self) -> io::Result<()> {
        for p in &self.processes {
            p.kill()?;
        }
        Ok(())
    }

    /// Detach all processes in the pipeline.
    ///
    /// After detaching, the processes will not be waited for in drop.
    pub fn detach(&self) {
        for p in &self.processes {
            p.detach();
        }
    }

    /// Poll all processes for completion without blocking.
    ///
    /// Returns `Some(exit_status)` of the last process if all processes have finished, or
    /// `None` if any process is still running. If no processes have been started (empty
    /// pipeline), returns `Some` with a successful exit status.
    pub fn poll(&self) -> Option<ExitStatus> {
        let mut status = Some(ExitStatus::from_raw(0));
        for p in &self.processes {
            status = Some(p.poll()?);
        }
        status
    }

    /// Like [`wait`](Self::wait), but with a timeout.
    ///
    /// Returns `Ok(None)` if the processes don't finish within the given duration.
    pub fn wait_timeout(&self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        let deadline = Instant::now() + timeout;
        let mut status = ExitStatus::from_raw(0);
        for p in &self.processes {
            match p.wait_timeout(deadline.saturating_duration_since(Instant::now()))? {
                Some(s) => status = s,
                None => return Ok(None),
            }
        }
        Ok(Some(status))
    }

    /// Closes the pipe ends, waits for all processes to finish, and returns the exit
    /// status of the last process.
    pub fn join(mut self) -> io::Result<ExitStatus> {
        self.communicate()?.read()?;
        let status = self.wait()?;
        if self.check_success && !status.success() {
            return Err(io::Error::other(format!("command failed: {status}")));
        }
        Ok(status)
    }

    /// Like [`join`](Self::join), but with a timeout.
    ///
    /// Returns an error of kind `ErrorKind::TimedOut` if the processes don't finish
    /// within the given duration.
    pub fn join_timeout(mut self, timeout: Duration) -> io::Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        self.communicate()?.limit_time(timeout).read()?;
        let status = self
            .wait_timeout(deadline.saturating_duration_since(Instant::now()))?
            .ok_or_else(|| io::Error::from(ErrorKind::TimedOut))?;
        if self.check_success && !status.success() {
            return Err(io::Error::other(format!("command failed: {status}")));
        }
        Ok(status)
    }

    /// Captures the output and waits for the process(es) to finish.
    ///
    /// Only streams that were redirected to a pipe will produce data; non-piped streams
    /// will result in empty bytes in `Capture`.
    pub fn capture(mut self) -> io::Result<Capture> {
        let mut comm = self.communicate()?;
        let (stdout, stderr) = comm.read()?;
        let capture = Capture {
            stdout,
            stderr,
            exit_status: self.wait()?,
        };
        if self.check_success && !capture.success() {
            return Err(io::Error::other(format!(
                "command failed: {}",
                capture.exit_status
            )));
        }
        Ok(capture)
    }

    /// Like [`capture`](Self::capture), but with a timeout.
    ///
    /// Returns an error of kind `ErrorKind::TimedOut` if the processes don't finish
    /// within the given duration.
    pub fn capture_timeout(mut self, timeout: Duration) -> io::Result<Capture> {
        let deadline = Instant::now() + timeout;
        let mut comm = self.communicate()?.limit_time(timeout);
        let (stdout, stderr) = comm.read()?;
        let exit_status = self
            .wait_timeout(deadline.saturating_duration_since(Instant::now()))?
            .ok_or_else(|| io::Error::from(ErrorKind::TimedOut))?;
        let capture = Capture {
            stdout,
            stderr,
            exit_status,
        };
        if self.check_success && !capture.success() {
            return Err(io::Error::other(format!(
                "command failed: {}",
                capture.exit_status
            )));
        }
        Ok(capture)
    }
}

#[derive(Debug)]
pub(crate) struct ReadAdapter(pub(crate) Job);

impl Read for ReadAdapter {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.stdout.as_mut().unwrap().read(buf)
    }
}

#[derive(Debug)]
pub(crate) struct ReadErrAdapter(pub(crate) Job);

impl Read for ReadErrAdapter {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.stderr.as_mut().unwrap().read(buf)
    }
}

#[derive(Debug)]
pub(crate) struct WriteAdapter(pub(crate) Job);

impl Write for WriteAdapter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.stdin.as_mut().unwrap().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.stdin.as_mut().unwrap().flush()
    }
}
