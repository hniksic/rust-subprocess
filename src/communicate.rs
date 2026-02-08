use std::fs::File;
use std::io::{self, Write};
use std::time::{Duration, Instant};

#[cfg(unix)]
mod posix {
    use crate::posix;
    use std::cmp::min;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::time::Instant;

    fn as_pollfd(f: Option<&File>, for_read: bool) -> posix::PollFd<'_> {
        let events = if for_read {
            posix::POLLIN
        } else {
            posix::POLLOUT
        };
        posix::PollFd::new(f, events)
    }

    fn maybe_poll(
        fin: Option<&File>,
        fout: Option<&File>,
        ferr: Option<&File>,
        deadline: Option<Instant>,
    ) -> io::Result<(bool, bool, bool)> {
        // Polling is needed to prevent deadlock when interacting with multiple streams,
        // and for timeout.  If we're interacting with a single stream without timeout, we
        // can skip the actual poll() syscall and just tell the caller to go ahead with
        // reading/writing.
        if deadline.is_none() {
            match (&fin, &fout, &ferr) {
                (None, None, Some(..)) => return Ok((false, false, true)),
                (None, Some(..), None) => return Ok((false, true, false)),
                (Some(..), None, None) => return Ok((true, false, false)),
                _ => (),
            }
        }
        let timeout = deadline.map(|d| d.saturating_duration_since(Instant::now()));

        let mut fds = [
            as_pollfd(fin, false),
            as_pollfd(fout, true),
            as_pollfd(ferr, true),
        ];
        posix::poll(&mut fds, timeout)?;

        Ok((
            fds[0].test(posix::POLLOUT | posix::POLLHUP),
            fds[1].test(posix::POLLIN | posix::POLLHUP),
            fds[2].test(posix::POLLIN | posix::POLLHUP),
        ))
    }

    #[derive(Debug)]
    pub struct RawCommunicator<I> {
        stdin: Option<File>,
        stdout: Option<File>,
        stderr: Option<File>,
        input_data: I,
        input_start: usize,
    }

    impl<I: AsRef<[u8]>> RawCommunicator<I> {
        pub fn new(
            stdin: Option<File>,
            stdout: Option<File>,
            stderr: Option<File>,
            input_data: I,
        ) -> RawCommunicator<I> {
            RawCommunicator {
                stdin,
                stdout,
                stderr,
                input_data,
                input_start: 0,
            }
        }

        fn do_read(
            source_ref: &mut Option<&File>,
            dest: &mut Vec<u8>,
            size_limit: Option<usize>,
            total_read: usize,
        ) -> io::Result<()> {
            let mut buf = &mut [0u8; 4096][..];
            if let Some(size_limit) = size_limit {
                if total_read >= size_limit {
                    return Ok(());
                }
                if size_limit - total_read < buf.len() {
                    buf = &mut buf[0..size_limit - total_read];
                }
            }
            let n = source_ref.unwrap().read(buf)?;
            if n != 0 {
                dest.extend_from_slice(&buf[..n]);
            } else {
                *source_ref = None;
            }
            Ok(())
        }

        pub fn read(
            &mut self,
            deadline: Option<Instant>,
            size_limit: Option<usize>,
            outret: &mut Option<Vec<u8>>,
            errret: &mut Option<Vec<u8>>,
        ) -> io::Result<()> {
            // Note: chunk size for writing must be smaller than the pipe buffer size.  A
            // large enough write to a pipe deadlocks despite polling.
            const WRITE_SIZE: usize = 4096;

            let outvec = if self.stdout.is_some() {
                outret.insert(vec![])
            } else {
                &mut vec![]
            };
            let errvec = if self.stderr.is_some() {
                errret.insert(vec![])
            } else {
                &mut vec![]
            };
            let mut stdout_live = self.stdout.as_ref();
            let mut stderr_live = self.stderr.as_ref();

            loop {
                let total = outvec.len() + errvec.len();
                if let Some(size_limit) = size_limit
                    && total >= size_limit
                {
                    break;
                }

                if let (None, None, None) = (self.stdin.as_ref(), stdout_live, stderr_live) {
                    // When no stream remains, we are done.
                    break;
                }

                let (in_ready, out_ready, err_ready) =
                    maybe_poll(self.stdin.as_ref(), stdout_live, stderr_live, deadline)?;
                if !in_ready && !out_ready && !err_ready {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "timeout"));
                }
                if in_ready {
                    let remaining = &self.input_data.as_ref()[self.input_start..];
                    let chunk = &remaining[..min(WRITE_SIZE, remaining.len())];
                    let n = self.stdin.as_ref().unwrap().write(chunk)?;
                    self.input_start += n;
                    if self.input_start >= self.input_data.as_ref().len() {
                        // close stdin when done writing, so the child receives EOF
                        self.stdin.take();
                    }
                }
                if out_ready {
                    Self::do_read(&mut stdout_live, outvec, size_limit, total)?;
                }
                if err_ready {
                    let total = outvec.len() + errvec.len();
                    Self::do_read(&mut stderr_live, errvec, size_limit, total)?;
                }
            }

            Ok(())
        }
    }
}

#[cfg(windows)]
mod win32 {
    use crate::spawn::StandardStream;
    use crate::win32::{
        PendingRead, PendingWrite, ReadFileOverlapped, WaitForMultipleObjects, WaitResult,
        WriteFileOverlapped,
    };
    use std::cmp::min;
    use std::fs::File;
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::time::Instant;

    const BUFFER_SIZE: usize = 4096;

    /// Wait for I/O completion on pending operations. Analogous to Unix maybe_poll().
    ///
    /// Takes references to pending operations and waits for any to complete.
    /// Returns `Ok(Some(stream))` indicating which completed, `Ok(None)` on timeout,
    /// or `Err` if the syscall fails.
    fn wait_for_io(
        stdin_pending: Option<&PendingWrite>,
        stdout_pending: Option<&PendingRead>,
        stderr_pending: Option<&PendingRead>,
        deadline: Option<Instant>,
    ) -> io::Result<StandardStream> {
        let mut handles = Vec::with_capacity(3);
        let mut streams = Vec::with_capacity(3);

        if let Some(p) = stdin_pending {
            handles.push(p.event().as_raw_handle());
            streams.push(StandardStream::Input);
        }
        if let Some(p) = stdout_pending {
            handles.push(p.event().as_raw_handle());
            streams.push(StandardStream::Output);
        }
        if let Some(p) = stderr_pending {
            handles.push(p.event().as_raw_handle());
            streams.push(StandardStream::Error);
        }
        assert!(!handles.is_empty());
        let timeout = deadline.map(|d| d.saturating_duration_since(Instant::now()));

        match WaitForMultipleObjects(&handles, timeout)? {
            WaitResult::Timeout => Err(io::ErrorKind::TimedOut.into()),
            WaitResult::Object(idx) => Ok(streams[idx]),
        }
    }

    /// Start a write operation.
    /// Returns Ok(true) if completed immediately, Ok(false) if pending.
    fn start_write(
        file: &File,
        pending: &mut Option<PendingWrite>,
        data: &[u8],
    ) -> io::Result<bool> {
        let chunk_size = min(BUFFER_SIZE, data.len());
        let new = WriteFileOverlapped(file.as_raw_handle(), &data[..chunk_size])?;
        Ok(pending.insert(new).is_ready())
    }

    /// Start a read operation.
    /// Returns Ok(true) if completed immediately, Ok(false) if pending.
    fn start_read(
        file: &File,
        pending: &mut Option<PendingRead>,
        read_size: usize,
    ) -> io::Result<bool> {
        let new = ReadFileOverlapped(file.as_raw_handle(), read_size)?;
        Ok(pending.insert(new).is_ready())
    }

    /// Complete a read operation and append data to dest.
    /// Returns Ok(true) if EOF was reached, Ok(false) otherwise.
    fn complete_read(mut pending: PendingRead, dest: &mut Vec<u8>) -> io::Result<bool> {
        let data = pending.complete()?;
        dest.extend_from_slice(data);
        Ok(data.is_empty())
    }

    #[derive(Debug)]
    pub struct RawCommunicator<I> {
        stdin: Option<File>,
        stdout: Option<File>,
        stderr: Option<File>,
        stdin_pending: Option<PendingWrite>,
        stdout_pending: Option<PendingRead>,
        stderr_pending: Option<PendingRead>,
        input_data: I,
        input_start: usize,
    }

    impl<I: AsRef<[u8]>> RawCommunicator<I> {
        pub fn new(
            stdin: Option<File>,
            stdout: Option<File>,
            stderr: Option<File>,
            input_data: I,
        ) -> RawCommunicator<I> {
            RawCommunicator {
                stdin,
                stdout,
                stderr,
                stdin_pending: None,
                stdout_pending: None,
                stderr_pending: None,
                input_data,
                input_start: 0,
            }
        }

        pub fn read(
            &mut self,
            deadline: Option<Instant>,
            size_limit: Option<usize>,
            outret: &mut Option<Vec<u8>>,
            errret: &mut Option<Vec<u8>>,
        ) -> io::Result<()> {
            // Note: size_limit enforcement is approximate on Windows when capturing both stdout
            // and stderr. On Unix, poll() signals readiness and we control how much to read. On
            // Windows, completion-based I/O means data is already in our buffer when we find out
            // about it. If both streams complete simultaneously, each may contribute a full
            // buffer before we can enforce the limit. We tried tracking partially-consumed
            // buffers to enforce strict limits, but the complexity wasn't worth it for a feature
            // whose intent is "don't read megabytes when I asked for kilobytes". The overshoot
            // is bounded by ~2x BUFFER_SIZE.

            let outvec = if self.stdout.is_some() {
                outret.insert(vec![])
            } else {
                &mut vec![]
            };
            let errvec = if self.stderr.is_some() {
                errret.insert(vec![])
            } else {
                &mut vec![]
            };
            // cleared after EOF
            let mut stdout_live = self.stdout.as_ref();
            let mut stderr_live = self.stderr.as_ref();

            loop {
                let total = outvec.len() + errvec.len();
                if let Some(size_limit) = size_limit
                    && total >= size_limit
                {
                    break;
                }
                if let (None, None, None) = (self.stdin.as_ref(), stdout_live, stderr_live) {
                    // When no stream remains, we are done.
                    break;
                }

                // Start I/O operations and track which completed immediately
                let mut in_ready = false;
                let mut out_ready = false;
                let mut err_ready = false;

                if let Some(ref stdin) = self.stdin
                    && self.stdin_pending.is_none()
                {
                    let remaining = &self.input_data.as_ref()[self.input_start..];
                    in_ready = start_write(stdin, &mut self.stdin_pending, remaining)?;
                }
                let read_size = size_limit
                    .map(|l| l.saturating_sub(total))
                    .unwrap_or(BUFFER_SIZE)
                    .min(BUFFER_SIZE);
                if let Some(stdout) = stdout_live
                    && self.stdout_pending.is_none()
                {
                    out_ready = start_read(stdout, &mut self.stdout_pending, read_size)?;
                }
                if let Some(stderr) = stderr_live
                    && self.stderr_pending.is_none()
                {
                    err_ready = start_read(stderr, &mut self.stderr_pending, read_size)?;
                }

                // If nothing completed immediately, wait for pending operations
                if !in_ready && !out_ready && !err_ready {
                    match wait_for_io(
                        self.stdin_pending.as_ref(),
                        self.stdout_pending.as_ref(),
                        self.stderr_pending.as_ref(),
                        deadline,
                    )? {
                        StandardStream::Input => in_ready = true,
                        StandardStream::Output => out_ready = true,
                        StandardStream::Error => err_ready = true,
                    }
                }

                // Complete operations and process data
                if in_ready {
                    let nwritten = self.stdin_pending.take().unwrap().complete()? as usize;
                    self.input_start += nwritten;
                    if self.input_start >= self.input_data.as_ref().len() {
                        // close stdin when done writing, so the child receives EOF
                        self.stdin.take();
                    }
                }
                if out_ready && complete_read(self.stdout_pending.take().unwrap(), outvec)? {
                    stdout_live = None;
                }
                if err_ready && complete_read(self.stderr_pending.take().unwrap(), errvec)? {
                    stderr_live = None;
                }
            }

            Ok(())
        }
    }
}

#[cfg(unix)]
use posix::RawCommunicator;
#[cfg(windows)]
use win32::RawCommunicator;

/// Wrapper around boxed input data that implements `AsRef<[u8]>`.
struct BoxedInput(Box<dyn AsRef<[u8]> + Send + Sync>);

impl AsRef<[u8]> for BoxedInput {
    fn as_ref(&self) -> &[u8] {
        (*self.0).as_ref()
    }
}

/// Send input to a subprocess and capture its output, without deadlock.
///
/// `Communicator` writes the provided input data to the subprocess's stdin (which is then
/// closed), while simultaneously reading its stdout and stderr until end-of-file.  This
/// parallel operation prevents deadlock that would occur if the subprocess produces output
/// while waiting for more input.
///
/// Create a `Communicator` by calling [`Job::communicate`], then call [`read`] or
/// [`read_string`] to perform the data exchange.
///
/// [`Job::communicate`]: crate::Job::communicate
/// [`read`]: #method.read
/// [`read_string`]: #method.read_string
#[must_use]
pub struct Communicator {
    inner: RawCommunicator<BoxedInput>,
    size_limit: Option<usize>,
    time_limit: Option<Duration>,
}

impl std::fmt::Debug for Communicator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Communicator")
            .field("size_limit", &self.size_limit)
            .field("time_limit", &self.time_limit)
            .finish_non_exhaustive()
    }
}

impl Communicator {
    pub(crate) fn new(
        stdin: Option<File>,
        stdout: Option<File>,
        stderr: Option<File>,
        input_data: impl AsRef<[u8]> + Send + Sync + 'static,
    ) -> Communicator {
        Communicator {
            inner: RawCommunicator::new(stdin, stdout, stderr, BoxedInput(Box::new(input_data))),
            size_limit: None,
            time_limit: None,
        }
    }

    /// Communicate with the subprocess, writing captured data to the provided writers.
    ///
    /// This will write input data to the subprocess's standard input and simultaneously read
    /// its standard output and error, writing any captured data to the provided writers.
    /// Data is written to the writers regardless of whether the read succeeds or fails,
    /// so the caller retains partial data on error.
    ///
    /// By default `read_to()` will read all data until end-of-file.
    ///
    /// If `limit_time` has been called, the method will read for no more than the specified
    /// duration.  In case of timeout, an error of kind `io::ErrorKind::TimedOut` is returned.
    /// Communication may be resumed after the timeout by calling `read_to()` again.
    ///
    /// If `limit_size` has been called, it will limit the allocation done by this method.  If
    /// the subprocess provides more data than the limit specifies, `read_to()` will
    /// successfully return as much data as specified by the limit.  (It might internally read
    /// a bit more from the subprocess, but the data will remain available for future reads.)
    /// Subsequent data can be retrieved by calling `read_to()` again, which can be repeated
    /// until `read_to()` writes no data, which marks EOF.
    ///
    /// Note that this method does not wait for the subprocess to finish, only to close its
    /// output/error streams.  It is rare but possible for the program to continue running
    /// after having closed the streams, in which case `Process::Drop` will wait for it
    /// to finish.  If such a wait is undesirable, it can be prevented by waiting
    /// explicitly using `wait()`, by detaching the process using `detach()`, or by
    /// terminating it with `terminate()`.
    ///
    /// # Errors
    ///
    /// * `Err(io::Error)` if a system call fails.  In case of timeout, the error kind will
    ///   be `ErrorKind::TimedOut`.
    pub fn read_to(&mut self, mut stdout: impl Write, mut stderr: impl Write) -> io::Result<()> {
        let deadline = self.time_limit.map(|timeout| Instant::now() + timeout);
        let mut outvec = None;
        let mut errvec = None;

        let result = self
            .inner
            .read(deadline, self.size_limit, &mut outvec, &mut errvec);

        let mut flush = Ok(());
        if let Some(out) = outvec
            && let Err(e) = stdout.write_all(&out)
        {
            flush = Err(e);
        }
        if let Some(err) = errvec
            && let Err(e) = stderr.write_all(&err)
            && flush.is_ok()
        {
            flush = Err(e);
        }
        result.and(flush)
    }

    /// Communicate with the subprocess, return the contents of its standard output and error.
    ///
    /// This will write input data to the subprocess's standard input and simultaneously read
    /// its standard output and error.  The output and error contents are returned as a pair
    /// of `Vec<u8>`.  An empty `Vec` means the stream was not redirected to a pipe, or that
    /// no data was produced.
    ///
    /// By default `read()` will read all data until end-of-file.
    ///
    /// If `limit_time` has been called, the method will read for no more than the specified
    /// duration.  In case of timeout, an error of kind `io::ErrorKind::TimedOut` is returned.
    /// Communication may be resumed after the timeout by calling `read()` again.
    ///
    /// If `limit_size` has been called, it will limit the allocation done by this method.  If
    /// the subprocess provides more data than the limit specifies, `read()` will successfully
    /// return as much data as specified by the limit.  (It might internally read a bit more
    /// from the subprocess, but the data will remain available for future reads.)  Subsequent
    /// data can be retrieved by calling `read()` again, which can be repeated until `read()`
    /// returns all-empty data, which marks EOF.
    ///
    /// Note that this method does not wait for the subprocess to finish, only to close its
    /// output/error streams.  It is rare but possible for the program to continue running
    /// after having closed the streams, in which case `Process::Drop` will wait for it
    /// to finish.  If such a wait is undesirable, it can be prevented by waiting
    /// explicitly using `wait()`, by detaching the process using `detach()`, or by
    /// terminating it with `terminate()`.
    ///
    /// # Errors
    ///
    /// * `Err(io::Error)` if a system call fails.  In case of timeout, the error kind will
    ///   be `ErrorKind::TimedOut`.
    pub fn read(&mut self) -> io::Result<(Vec<u8>, Vec<u8>)> {
        let mut out = vec![];
        let mut err = vec![];
        self.read_to(&mut out, &mut err)?;
        Ok((out, err))
    }

    /// Return the subprocess's output and error contents as strings.
    ///
    /// Like `read()`, but returns strings instead of byte vectors.  Invalid UTF-8 sequences,
    /// if found, are replaced with the `U+FFFD` Unicode replacement character.
    pub fn read_string(&mut self) -> io::Result<(String, String)> {
        let (out, err) = self.read()?;
        Ok((from_utf8_lossy(out), from_utf8_lossy(err)))
    }

    /// Limit the amount of data the next `read()` will read from the subprocess.
    ///
    /// On Windows, when capturing both stdout and stderr, the limit is approximate
    /// and may be exceeded by several kilobytes.
    pub fn limit_size(mut self, size: usize) -> Communicator {
        self.size_limit = Some(size);
        self
    }

    /// Limit the amount of time the next `read()` will spend reading from the subprocess.
    pub fn limit_time(mut self, time: Duration) -> Communicator {
        self.time_limit = Some(time);
        self
    }
}

/// Like String::from_utf8_lossy(), but takes `Vec<u8>` and reuses its storage if possible.
fn from_utf8_lossy(v: Vec<u8>) -> String {
    match String::from_utf8(v) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into(),
    }
}
