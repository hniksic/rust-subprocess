use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, ErrorKind};
use std::time::{Duration, Instant};

#[cfg(unix)]
mod raw {
    use crate::posix;
    use std::cmp::min;
    use std::collections::VecDeque;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::time::{Duration, Instant};

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
        // Polling is needed to prevent deadlock when interacting with
        // multiple streams, and for timeout.  If we're interacting with a
        // single stream without timeout, we can skip the actual poll()
        // syscall and just tell the caller to go ahead with reading/writing.
        if deadline.is_none() {
            match (&fin, &fout, &ferr) {
                (None, None, Some(..)) => return Ok((false, false, true)),
                (None, Some(..), None) => return Ok((false, true, false)),
                (Some(..), None, None) => return Ok((true, false, false)),
                _ => (),
            }
        }

        let timeout = deadline.map(|deadline| {
            let now = Instant::now();
            if now >= deadline {
                Duration::from_secs(0)
            } else {
                deadline - now
            }
        });

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
    pub struct RawCommunicator {
        stdin: Option<File>,
        stdout: Option<File>,
        stderr: Option<File>,
        input_data: VecDeque<u8>,
    }

    impl RawCommunicator {
        pub fn new(
            stdin: Option<File>,
            stdout: Option<File>,
            stderr: Option<File>,
            input_data: Option<Vec<u8>>,
        ) -> RawCommunicator {
            RawCommunicator {
                stdin,
                stdout,
                stderr,
                input_data: VecDeque::from(input_data.unwrap_or_default()),
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

        fn read_into(
            &mut self,
            deadline: Option<Instant>,
            size_limit: Option<usize>,
            outvec: &mut Vec<u8>,
            errvec: &mut Vec<u8>,
        ) -> io::Result<()> {
            // Note: chunk size for writing must be smaller than the pipe buffer
            // size.  A large enough write to a pipe deadlocks despite polling.
            const WRITE_SIZE: usize = 4096;

            let mut stdout_ref = self.stdout.as_ref();
            let mut stderr_ref = self.stderr.as_ref();

            loop {
                if let Some(size_limit) = size_limit
                    && outvec.len() + errvec.len() >= size_limit
                {
                    break;
                }

                if let (None, None, None) = (self.stdin.as_ref(), stdout_ref, stderr_ref) {
                    // When no stream remains, we are done.
                    break;
                }

                let (in_ready, out_ready, err_ready) =
                    maybe_poll(self.stdin.as_ref(), stdout_ref, stderr_ref, deadline)?;
                if !in_ready && !out_ready && !err_ready {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "timeout"));
                }
                if in_ready {
                    // make_contiguous() is a no-op here: we start from a Vec and only
                    // drain from the front, so the data never wraps around.
                    let input = self.input_data.make_contiguous();
                    let chunk = &input[..min(WRITE_SIZE, input.len())];
                    let n = self.stdin.as_ref().unwrap().write(chunk)?;
                    self.input_data.drain(..n);
                    if self.input_data.is_empty() {
                        // close stdin when done writing, so the child receives EOF
                        self.stdin.take();
                    }
                }
                if out_ready {
                    RawCommunicator::do_read(
                        &mut stdout_ref,
                        outvec,
                        size_limit,
                        outvec.len() + errvec.len(),
                    )?;
                }
                if err_ready {
                    RawCommunicator::do_read(
                        &mut stderr_ref,
                        errvec,
                        size_limit,
                        outvec.len() + errvec.len(),
                    )?;
                }
            }

            Ok(())
        }

        pub fn read(
            &mut self,
            deadline: Option<Instant>,
            size_limit: Option<usize>,
        ) -> (Option<io::Error>, (Option<Vec<u8>>, Option<Vec<u8>>)) {
            let mut outvec = vec![];
            let mut errvec = vec![];

            let err = self
                .read_into(deadline, size_limit, &mut outvec, &mut errvec)
                .err();
            let output = (
                self.stdout.as_ref().map(|_| outvec),
                self.stderr.as_ref().map(|_| errvec),
            );
            (err, output)
        }
    }
}

#[cfg(windows)]
mod raw {
    use crate::win32::{
        PendingRead, PendingWrite, ReadFileOverlapped, WaitForMultipleObjects, WaitResult,
        WriteFileOverlapped,
    };
    use std::cmp::min;
    use std::collections::VecDeque;
    use std::fs::File;
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::time::{Duration, Instant};

    const BUFFER_SIZE: usize = 4096;

    /// Identifies which stream became ready after waiting.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ReadyStream {
        Stdin,
        Stdout,
        Stderr,
    }

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
    ) -> io::Result<ReadyStream> {
        let mut handles = Vec::with_capacity(3);
        let mut streams = Vec::with_capacity(3);

        if let Some(p) = stdin_pending {
            handles.push(p.event().as_raw_handle());
            streams.push(ReadyStream::Stdin);
        }
        if let Some(p) = stdout_pending {
            handles.push(p.event().as_raw_handle());
            streams.push(ReadyStream::Stdout);
        }
        if let Some(p) = stderr_pending {
            handles.push(p.event().as_raw_handle());
            streams.push(ReadyStream::Stderr);
        }
        assert!(!handles.is_empty());

        let timeout = deadline.map(|d| {
            let now = Instant::now();
            if now >= d {
                Duration::from_secs(0)
            } else {
                d - now
            }
        });

        match WaitForMultipleObjects(&handles, timeout)? {
            WaitResult::Timeout => Err(io::Error::new(io::ErrorKind::TimedOut, "timeout")),
            WaitResult::Object(idx) => Ok(streams[idx]),
        }
    }

    /// Start a read operation.
    /// Returns Ok(true) if completed immediately, Ok(false) if pending.
    fn start_write(
        file: &File,
        pending: &mut Option<PendingWrite>,
        data: &mut VecDeque<u8>,
    ) -> io::Result<bool> {
        // make_contiguous() is a no-op: we only drain from the front, so the data never
        // wraps around.
        let data = data.make_contiguous();
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
        if pending.complete()? == 0 {
            Ok(true)
        } else {
            dest.extend_from_slice(pending.data());
            Ok(false)
        }
    }

    fn complete_write(mut pending: PendingWrite, source: &mut VecDeque<u8>) -> io::Result<()> {
        let nwritten = pending.complete()? as usize;
        source.drain(..nwritten);
        Ok(())
    }

    #[derive(Debug)]
    pub struct RawCommunicator {
        stdin: Option<File>,
        stdout: Option<File>,
        stderr: Option<File>,
        stdin_pending: Option<PendingWrite>,
        stdout_pending: Option<PendingRead>,
        stderr_pending: Option<PendingRead>,
        input_data: VecDeque<u8>,
    }

    impl RawCommunicator {
        pub fn new(
            stdin: Option<File>,
            stdout: Option<File>,
            stderr: Option<File>,
            input_data: Option<Vec<u8>>,
        ) -> RawCommunicator {
            RawCommunicator {
                stdin,
                stdout,
                stderr,
                stdin_pending: None,
                stdout_pending: None,
                stderr_pending: None,
                input_data: VecDeque::from(input_data.unwrap_or_default()),
            }
        }

        fn read_into(
            &mut self,
            deadline: Option<Instant>,
            size_limit: Option<usize>,
            outvec: &mut Vec<u8>,
            errvec: &mut Vec<u8>,
        ) -> io::Result<()> {
            // Track whether streams have reached EOF (separate from self.stdout/stderr
            // which we keep for the return value)
            let mut stdout_eof = self.stdout.is_none();
            let mut stderr_eof = self.stderr.is_none();

            loop {
                if let Some(size_limit) = size_limit
                    && outvec.len() + errvec.len() >= size_limit
                {
                    break;
                }
                if self.stdin.is_none() && stdout_eof && stderr_eof {
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
                    in_ready = start_write(stdin, &mut self.stdin_pending, &mut self.input_data)?;
                }
                let read_size = size_limit
                    .map(|l| l.saturating_sub(outvec.len() + errvec.len()))
                    .unwrap_or(BUFFER_SIZE)
                    .min(BUFFER_SIZE);
                if !stdout_eof && self.stdout_pending.is_none() {
                    out_ready = start_read(
                        self.stdout.as_ref().unwrap(),
                        &mut self.stdout_pending,
                        read_size,
                    )?;
                }
                if !stderr_eof && self.stderr_pending.is_none() {
                    err_ready = start_read(
                        self.stderr.as_ref().unwrap(),
                        &mut self.stderr_pending,
                        read_size,
                    )?;
                }

                // If nothing completed immediately, wait for pending operations
                if !in_ready && !out_ready && !err_ready {
                    match wait_for_io(
                        self.stdin_pending.as_ref(),
                        self.stdout_pending.as_ref(),
                        self.stderr_pending.as_ref(),
                        deadline,
                    )? {
                        ReadyStream::Stdin => in_ready = true,
                        ReadyStream::Stdout => out_ready = true,
                        ReadyStream::Stderr => err_ready = true,
                    }
                }

                // Complete operations and process data
                if in_ready {
                    complete_write(self.stdin_pending.take().unwrap(), &mut self.input_data)?;
                    if self.input_data.is_empty() {
                        // close stdin when done writing, so the child receives EOF
                        self.stdin.take();
                    }
                }
                if out_ready {
                    stdout_eof = complete_read(self.stdout_pending.take().unwrap(), outvec)?;
                }
                if err_ready {
                    stderr_eof = complete_read(self.stderr_pending.take().unwrap(), errvec)?;
                }
            }

            Ok(())
        }

        pub fn read(
            &mut self,
            deadline: Option<Instant>,
            size_limit: Option<usize>,
        ) -> (Option<io::Error>, (Option<Vec<u8>>, Option<Vec<u8>>)) {
            let mut outvec = vec![];
            let mut errvec = vec![];

            let err = self
                .read_into(deadline, size_limit, &mut outvec, &mut errvec)
                .err();
            let output = (
                self.stdout.as_ref().map(|_| outvec),
                self.stderr.as_ref().map(|_| errvec),
            );
            (err, output)
        }
    }
}

use raw::RawCommunicator;

/// Send input to a subprocess and capture its output, without deadlock.
///
/// `Communicator` writes the provided input data to the subprocess's stdin (which is then
/// closed), while simultaneously reading its stdout and stderr until end-of-file.  This
/// parallel operation prevents deadlock that would occur if the subprocess produces output
/// while waiting for more input.
///
/// Create a `Communicator` by calling [`Popen::communicate_start`], then call [`read`] or
/// [`read_string`] to perform the data exchange.
///
/// [`Popen::communicate_start`]: struct.Popen.html#method.communicate_start
/// [`read`]: #method.read
/// [`read_string`]: #method.read_string
#[must_use]
#[derive(Debug)]
pub struct Communicator {
    inner: RawCommunicator,
    size_limit: Option<usize>,
    time_limit: Option<Duration>,
}

impl Communicator {
    fn new(
        stdin: Option<File>,
        stdout: Option<File>,
        stderr: Option<File>,
        input_data: Option<Vec<u8>>,
    ) -> Communicator {
        Communicator {
            inner: RawCommunicator::new(stdin, stdout, stderr, input_data),
            size_limit: None,
            time_limit: None,
        }
    }

    /// Communicate with the subprocess, return the contents of its standard output and error.
    ///
    /// This will write input data to the subprocess's standard input and simultaneously read
    /// its standard output and error.  The output and error contents are returned as a pair
    /// of `Option<Vec>`.  The `None` options correspond to streams not specified as
    /// `Redirection::Pipe` when creating the subprocess.
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
    /// after having closed the streams, in which case `Popen::Drop` will wait for it to
    /// finish.  If such a wait is undesirable, it can be prevented by waiting explicitly
    /// using `wait()`, by detaching the process using `detach()`, or by terminating it with
    /// `terminate()`.
    ///
    /// # Panics
    ///
    /// If `input_data` is provided and `stdin` was not redirected to a pipe.  Also, if
    /// `input_data` is not provided and `stdin` was redirected to a pipe.
    ///
    /// # Errors
    ///
    /// * `Err(CommunicateError)` if a system call fails.  In case of timeout, the underlying
    ///   error kind will be `ErrorKind::TimedOut`.
    ///
    /// Regardless of the nature of the error, the content prior to the error can be retrieved
    /// using the [`capture`] attribute of the error.
    ///
    /// [`capture`]: struct.CommunicateError.html#structfield.capture
    pub fn read(&mut self) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), CommunicateError> {
        let deadline = self.time_limit.map(|timeout| Instant::now() + timeout);
        match self.inner.read(deadline, self.size_limit) {
            (None, capture) => Ok(capture),
            (Some(error), capture) => Err(CommunicateError { error, capture }),
        }
    }

    /// Return the subprocess's output and error contents as strings.
    ///
    /// Like `read()`, but returns strings instead of byte vectors.  Invalid UTF-8 sequences,
    /// if found, are replaced with the `U+FFFD` Unicode replacement character.
    pub fn read_string(&mut self) -> Result<(Option<String>, Option<String>), CommunicateError> {
        let (o, e) = self.read()?;
        Ok((o.map(from_utf8_lossy), e.map(from_utf8_lossy)))
    }

    /// Limit the amount of data the next `read()` will read from the subprocess.
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

pub fn communicate(
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
    input_data: Option<Vec<u8>>,
) -> Communicator {
    if stdin.is_some() {
        input_data
            .as_ref()
            .expect("must provide input to redirected stdin");
    } else {
        assert!(
            input_data.as_ref().is_none(),
            "cannot provide input to non-redirected stdin"
        );
    }
    Communicator::new(stdin, stdout, stderr, input_data)
}

/// Error during communication.
///
/// It holds the underlying `io::Error` in the `error` field, and also provides the data
/// captured before the error was encountered in the `capture` field.
///
/// The error description and cause are taken from the underlying IO error.
#[derive(Debug)]
pub struct CommunicateError {
    /// The underlying `io::Error`.
    pub error: io::Error,
    /// The data captured before the error was encountered.
    pub capture: (Option<Vec<u8>>, Option<Vec<u8>>),
}

impl CommunicateError {
    /// Returns the corresponding IO `ErrorKind` for this error.
    ///
    /// Equivalent to `self.error.kind()`.
    pub fn kind(&self) -> ErrorKind {
        self.error.kind()
    }
}

impl Error for CommunicateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.error.source()
    }
}

impl fmt::Display for CommunicateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}
