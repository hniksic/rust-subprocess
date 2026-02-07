use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::ops::BitOr;
use std::path::Path;
use std::sync::Arc;

use crate::communicate::Communicator;
use crate::popen::{ExitStatus, Redirection};
use crate::process::Process;

use crate::exec::{
    Capture, Exec, InputRedirection, InputRedirectionKind, OutputRedirection, ReadAdapter,
    ReadErrAdapter, Started, WriteAdapter,
};

/// A builder for pipelines of subprocesses connected via pipes.
///
/// A pipeline is a sequence of two or more [`Exec`] commands connected via pipes.  Just
/// like in a Unix shell pipeline, each command receives standard input from the previous
/// command, and passes standard output to the next command.  Optionally, the standard
/// input of the first command can be provided from the outside, and the output of the
/// last command can be captured.
///
/// In most cases you do not need to create [`Pipeline`] instances directly; instead,
/// combine [`Exec`] instances using the `|` operator which produces `Pipeline`.
///
/// # Examples
///
/// Execute a pipeline and return the exit status of the last command:
///
/// ```no_run
/// # use subprocess::*;
/// # fn dummy() -> std::io::Result<()> {
/// let exit_status =
///   (Exec::shell("ls *.bak") | Exec::cmd("xargs").arg("rm")).join()?;
/// # Ok(())
/// # }
/// ```
///
/// Capture the pipeline's output:
///
/// ```no_run
/// # use subprocess::*;
/// # fn dummy() -> std::io::Result<()> {
/// let dir_checksum = {
///     Exec::shell("find . -type f") | Exec::cmd("sort") | Exec::cmd("sha1sum")
/// }.capture()?.stdout_str();
/// # Ok(())
/// # }
/// ```
#[must_use]
pub struct Pipeline {
    execs: Vec<Exec>,
    stdin: Redirection,
    stdout: Redirection,
    stderr: Redirection,
    stdin_data: Option<Vec<u8>>,
    check_success: bool,
    detached: bool,
    cwd: Option<OsString>,
    #[cfg(unix)]
    setpgid: bool,
}

impl Default for Pipeline {
    fn default() -> Pipeline {
        Pipeline::new()
    }
}

impl Pipeline {
    /// Creates a new empty pipeline.
    ///
    /// Use [`pipe`](Self::pipe) to add commands to the pipeline, or the `|` operator
    /// to combine `Exec` instances.
    ///
    /// An empty pipeline's `join()` returns success and `capture()` returns empty
    /// output. A single-command pipeline behaves like the command run on its own.
    pub fn new() -> Pipeline {
        Pipeline {
            execs: vec![],
            stdin: Redirection::None,
            stdout: Redirection::None,
            stderr: Redirection::None,
            stdin_data: None,
            check_success: false,
            detached: false,
            cwd: None,
            #[cfg(unix)]
            setpgid: false,
        }
    }

    /// Appends a command to the pipeline.
    ///
    /// This is the builder-style equivalent of the `|` operator.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use subprocess::*;
    /// # fn dummy() -> std::io::Result<()> {
    /// let output = Pipeline::new()
    ///     .pipe(Exec::cmd("echo").arg("hello world"))
    ///     .pipe(Exec::cmd("wc").arg("-w"))
    ///     .capture()?
    ///     .stdout_str();
    /// # Ok(())
    /// # }
    /// ```
    pub fn pipe(mut self, cmd: Exec) -> Pipeline {
        self.execs.push(cmd);
        self
    }

    /// Specifies how to set up the standard input of the first command in the pipeline.
    ///
    /// Argument can be:
    ///
    /// * a [`Redirection`];
    /// * a `File`, which is a shorthand for `Redirection::File(file)`;
    /// * a `Vec<u8>`, `&str`, or `&[u8]`, which will set up a `Redirection::Pipe`
    ///   for stdin, making sure that `capture` feeds that data into the standard
    ///   input of the subprocess.
    ///
    /// [`Redirection`]: enum.Redirection.html
    pub fn stdin(mut self, stdin: impl InputRedirection) -> Pipeline {
        match stdin.into_input_redirection() {
            InputRedirectionKind::AsRedirection(r) => self.stdin = r,
            InputRedirectionKind::FeedData(data) => {
                self.stdin = Redirection::Pipe;
                self.stdin_data = Some(data);
            }
        };
        self
    }

    /// Specifies how to set up the standard output of the last command in the pipeline.
    ///
    /// Argument can be:
    ///
    /// * a [`Redirection`];
    /// * a `File`, which is a shorthand for `Redirection::File(file)`.
    ///
    /// [`Redirection`]: enum.Redirection.html
    pub fn stdout(mut self, stdout: impl OutputRedirection) -> Pipeline {
        self.stdout = stdout.into_output_redirection();
        self
    }

    /// Specifies how to set up the standard error of all commands in the pipeline.
    ///
    /// Unlike `stdout()`, which only affects the last command in the pipeline, this
    /// affects all commands.  The difference is because standard output is piped from
    /// one command to the next, so only the output of the last command is "free".  In
    /// contrast, the standard errors are not connected to each other and can be
    /// configured *en masse*.
    ///
    /// Argument can be:
    ///
    /// * a [`Redirection`];
    /// * a `File`, which is a shorthand for `Redirection::File(file)`.
    ///
    /// All `Redirection` variants are meaningful:
    ///
    /// * `Redirection::None` - inherit from the parent (the default)
    /// * `Redirection::Pipe` - funnel stderr of all commands into stderr obtained
    ///   with `capture()` or `communicate()`
    /// * `Redirection::Merge` - redirect stderr to stdout, like `2>&1` for each
    ///   command
    /// * `Redirection::File(f)` / `Redirection::SharedFile(arc)` - redirect to a file
    /// * `Redirection::Null` - suppress stderr
    ///
    /// Note that this differs from the shell's `cmd1 | cmd2 2>file`, which only
    /// redirects stderr of the last command.  This method is equivalent to `(cmd1 |
    /// cmd2) 2>file`, but without the overhead of a subshell.
    ///
    /// If you pass `Redirection::Pipe`, the shared stderr read end
    /// will be available via [`Started::stderr`].
    ///
    /// [`Redirection`]: enum.Redirection.html
    pub fn stderr_all(mut self, stderr: impl OutputRedirection) -> Pipeline {
        self.stderr = stderr.into_output_redirection();
        self
    }

    /// If called, [`join`](Self::join) and [`capture`](Self::capture) will return
    /// an error if the last command in the pipeline exits with a non-zero status.
    pub fn checked(mut self) -> Pipeline {
        self.check_success = true;
        self
    }

    /// Specifies the current working directory for all commands in the pipeline.
    ///
    /// If unspecified, the current working directory is inherited from the parent.
    pub fn cwd(mut self, dir: impl AsRef<Path>) -> Pipeline {
        self.cwd = Some(dir.as_ref().as_os_str().to_owned());
        self
    }

    /// Specifies that the pipeline processes are initially detached.
    ///
    /// A detached pipeline means that we will not wait for the processes
    /// to finish when the objects that own them go out of scope.
    pub fn detached(mut self) -> Pipeline {
        self.detached = true;
        self
    }

    #[cfg(unix)]
    pub(crate) fn set_setpgid(&mut self, value: bool) {
        self.setpgid = value;
    }

    fn check_no_stdin_data(&self, meth: &str) {
        if self.stdin_data.is_some() {
            panic!("{} called with input data specified", meth);
        }
    }

    /// Convert pipeline-level stderr redirection into a per-command form, applying it
    /// to all commands. Returns the read end of the pipe if stderr was set to Pipe.
    fn setup_stderr(&mut self) -> io::Result<Option<File>> {
        let (redirection, stderr_read) =
            match std::mem::replace(&mut self.stderr, Redirection::None) {
                Redirection::None => return Ok(None),
                Redirection::Pipe => {
                    let (stderr_read, stderr_write) = crate::popen::make_pipe()?;
                    (
                        Redirection::SharedFile(Arc::new(stderr_write)),
                        Some(stderr_read),
                    )
                }
                Redirection::File(f) => (Redirection::SharedFile(Arc::new(f)), None),
                other => (other, None),
            };
        // unwrap(): after the above conversion, redirection to apply to each cmd is
        // SharedFile, Merge, or Null, all of which are infallible to clone.
        self.execs = self
            .execs
            .drain(..)
            .map(|cmd| cmd.stderr(redirection.try_clone().unwrap()))
            .collect();
        Ok(stderr_read)
    }

    // Terminators:

    /// Starts all commands in the pipeline and returns a [`Started`] with the running
    /// processes and their pipe ends.
    ///
    /// If some command fails to start, the remaining commands will not be started, and
    /// the appropriate error will be returned.  The commands that have already started
    /// will be waited to finish (but will probably exit immediately due to missing
    /// output), except for the ones for which `detached()` was called.  This is
    /// equivalent to what the shell does.
    pub fn start(mut self) -> io::Result<Started> {
        if self.execs.is_empty() {
            return Ok(Started {
                stdin: None,
                stdout: None,
                stderr: None,
                stdin_data: vec![],
                check_success: self.check_success,
                processes: vec![],
            });
        }

        if self.execs.first().unwrap().stdin_is_set() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "stdin of the first command is already redirected; \
                 use Pipeline::stdin() to redirect pipeline input",
            ));
        }
        if self.execs.last().unwrap().stdout_is_set() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "stdout of the last command is already redirected; \
                 use Pipeline::stdout() to redirect pipeline output",
            ));
        }

        #[cfg(unix)]
        if self.execs.iter().any(|e| e.setpgid_is_set()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "setpgid on individual commands in a pipeline is not \
                 supported; use Pipeline::setpgid() to put the pipeline \
                 in a process group",
            ));
        }

        let stderr = self.setup_stderr()?;

        if let Some(dir) = &self.cwd {
            self.execs = self.execs.into_iter().map(|cmd| cmd.cwd(dir)).collect();
        }
        if self.detached {
            self.execs = self.execs.into_iter().map(|cmd| cmd.detached()).collect();
        }

        let first_cmd = self.execs.remove(0);
        self.execs.insert(0, first_cmd.stdin(self.stdin));

        let last_cmd = self.execs.remove(self.execs.len() - 1);
        self.execs.push(last_cmd.stdout(self.stdout));

        let cnt = self.execs.len();
        let mut processes = Vec::<Process>::new();
        let mut pipeline_stdin = None;
        let mut pipeline_stdout = None;
        let mut prev_stdout: Option<File> = None;
        #[cfg(unix)]
        let mut first_pid: u32 = 0;

        for (idx, mut runner) in self.execs.into_iter().enumerate() {
            if let Some(prev_out) = prev_stdout.take() {
                runner = runner.stdin(prev_out);
            }
            if idx != cnt - 1 {
                runner = runner.stdout(Redirection::Pipe);
            }
            #[cfg(unix)]
            if self.setpgid {
                // No race condition: spawn() uses an exec-fail pipe,
                // so it blocks until the child has called setpgid and
                // exec'd. By the time we fork the second child, the
                // first child's group already exists.
                if idx == 0 {
                    runner.set_pgid_value(0);
                } else {
                    runner.set_pgid_value(first_pid);
                }
            }
            let result = runner.spawn()?;
            if idx == 0 {
                pipeline_stdin = result.stdin;
                #[cfg(unix)]
                if self.setpgid {
                    first_pid = result.process.pid();
                }
            }
            if idx == cnt - 1 {
                pipeline_stdout = result.stdout;
            } else {
                prev_stdout = result.stdout;
            }
            processes.push(result.process);
        }

        Ok(Started {
            stdin: pipeline_stdin,
            stdout: pipeline_stdout,
            stderr,
            stdin_data: self.stdin_data.unwrap_or_default(),
            check_success: self.check_success,
            processes,
        })
    }

    /// Starts the pipeline, waits for it to finish, and returns the exit status
    /// of the last command.
    pub fn join(self) -> io::Result<ExitStatus> {
        self.start()?.join()
    }

    /// Starts the pipeline and returns a value implementing the `Read` trait that reads
    /// from the standard output of the last command.
    ///
    /// This will automatically set up `stdout(Redirection::Pipe)`, so it is not necessary
    /// to do that beforehand.
    ///
    /// When the trait object is dropped, it will wait for the pipeline to finish.  If
    /// this is undesirable, use `detached()`.
    ///
    /// # Panics
    ///
    /// Panics if input data was specified with [`stdin`](Self::stdin).  Use
    /// [`capture`](Self::capture) or [`communicate`](Self::communicate) to both
    /// feed input and read output.
    pub fn stream_stdout(self) -> io::Result<impl Read> {
        self.check_no_stdin_data("stream_stdout");
        let handle = self.stdout(Redirection::Pipe).start()?;
        Ok(ReadAdapter(handle))
    }

    /// Starts the pipeline and returns a value implementing the `Read` trait that reads
    /// from the standard error of all commands in the pipeline.
    ///
    /// This will automatically set up `stderr_all(Redirection::Pipe)`, so it is not
    /// necessary to do that beforehand.
    ///
    /// Note that this redirects stderr of all commands in the pipeline, not just
    /// the last one.  This differs from the shell's `cmd1 | cmd2 2>file`, which
    /// only redirects stderr of the last command.  This method is equivalent to
    /// `(cmd1 | cmd2) 2>file`, but without the overhead of a subshell.
    ///
    /// When the trait object is dropped, it will wait for the pipeline to finish.  If
    /// this is undesirable, use `detached()`.
    ///
    /// # Panics
    ///
    /// Panics if input data was specified with [`stdin`](Self::stdin).  Use
    /// [`capture`](Self::capture) or [`communicate`](Self::communicate) to both
    /// feed input and read output.
    pub fn stream_stderr_all(self) -> io::Result<impl Read> {
        self.check_no_stdin_data("stream_stderr_all");
        let handle = self.stderr_all(Redirection::Pipe).start()?;
        Ok(ReadErrAdapter(handle))
    }

    /// Starts the pipeline and returns a value implementing the `Write` trait that writes
    /// to the standard input of the first command.
    ///
    /// This will automatically set up `stdin(Redirection::Pipe)`, so it is not necessary
    /// to do that beforehand.
    ///
    /// When the trait object is dropped, it will wait for the process to finish.  If this
    /// is undesirable, use `detached()`.
    ///
    /// # Panics
    ///
    /// Panics if input data was specified with [`stdin`](Self::stdin).
    pub fn stream_stdin(self) -> io::Result<impl Write> {
        self.check_no_stdin_data("stream_stdin");
        let handle = self.stdin(Redirection::Pipe).start()?;
        Ok(WriteAdapter(handle))
    }

    /// Starts the pipeline and returns a `Communicator` handle.
    ///
    /// Unless already configured, stdout and stderr are redirected to pipes so they
    /// can be read from the communicator. If you need different redirection
    /// (e.g. `stderr_all(Merge)`), set it up before calling this method and it will
    /// be preserved.
    ///
    /// Compared to `capture()`, this offers more choice in how communication is
    /// performed, such as read size limit and timeout.  Unlike `capture()`, this
    /// method doesn't wait for the pipeline to finish, effectively detaching it.
    pub fn communicate(mut self) -> io::Result<Communicator<Vec<u8>>> {
        self = self.detached();
        let setup_stdout = matches!(self.stdout, Redirection::None);
        let setup_stderr = matches!(self.stderr, Redirection::None);
        if setup_stdout {
            self = self.stdout(Redirection::Pipe);
        }
        if setup_stderr {
            self = self.stderr_all(Redirection::Pipe);
        }
        Ok(self.start()?.communicate())
    }

    /// Starts the pipeline, collects its standard output and error, and waits for all
    /// commands to finish.
    ///
    /// Unless already configured, stdout and stderr are redirected to pipes so they
    /// can be captured. If you need different redirection (e.g. `stderr_all(Merge)`),
    /// set it up before calling this method and it will be preserved.
    ///
    /// This method actually waits for the processes to finish, rather than simply
    /// waiting for the output to close.  If this is undesirable, use `detached()`.
    pub fn capture(mut self) -> io::Result<Capture> {
        let setup_stdout = matches!(self.stdout, Redirection::None);
        let setup_stderr = matches!(self.stderr, Redirection::None);
        if setup_stdout {
            self = self.stdout(Redirection::Pipe);
        }
        if setup_stderr {
            self = self.stderr_all(Redirection::Pipe);
        }
        self.start()?.capture()
    }

    /// Returns a copy of this `Pipeline`.
    ///
    /// This can fail if a `Redirection::File` is present because duplicating the
    /// underlying file descriptor is an OS operation that can fail.
    pub fn try_clone(&self) -> io::Result<Pipeline> {
        let execs = self
            .execs
            .iter()
            .map(|cmd| cmd.try_clone())
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Pipeline {
            execs,
            stdin: self.stdin.try_clone()?,
            stdout: self.stdout.try_clone()?,
            stderr: self.stderr.try_clone()?,
            stdin_data: self.stdin_data.clone(),
            check_success: self.check_success,
            detached: self.detached,
            cwd: self.cwd.clone(),
            #[cfg(unix)]
            setpgid: self.setpgid,
        })
    }
}

impl BitOr<Exec> for Pipeline {
    type Output = Pipeline;

    /// Append a command to the pipeline and return a new pipeline.
    fn bitor(self, rhs: Exec) -> Pipeline {
        self.pipe(rhs)
    }
}

impl BitOr for Pipeline {
    type Output = Pipeline;

    /// Append the commands from `rhs` to this pipeline.
    ///
    /// Other pipeline-level settings (cwd, stdout, etc.) from `rhs` are dropped -
    /// only its commands are taken.
    fn bitor(mut self, rhs: Pipeline) -> Pipeline {
        for exec in rhs.execs {
            self = self.pipe(exec);
        }
        self
    }
}

impl FromIterator<Exec> for Pipeline {
    /// Creates a pipeline from an iterator of commands.
    ///
    /// The iterator may yield any number of commands, including zero or one.
    /// An empty pipeline returns success on `join()` and empty output on
    /// `capture()`. A single-command pipeline behaves like running that
    /// command directly.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use subprocess::{Exec, Pipeline};
    ///
    /// let commands = vec![
    ///   Exec::shell("echo tset"),
    ///   Exec::shell("tr '[:lower:]' '[:upper:]'"),
    ///   Exec::shell("rev")
    /// ];
    ///
    /// let pipeline: Pipeline = commands.into_iter().collect();
    /// let output = pipeline.capture().unwrap().stdout_str();
    /// assert_eq!(output, "TEST\n");
    /// ```
    fn from_iter<I: IntoIterator<Item = Exec>>(iter: I) -> Self {
        Pipeline {
            execs: iter.into_iter().collect(),
            stdin: Redirection::None,
            stdout: Redirection::None,
            stderr: Redirection::None,
            stdin_data: None,
            check_success: false,
            detached: false,
            cwd: None,
            #[cfg(unix)]
            setpgid: false,
        }
    }
}

impl fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut args = vec![];
        for cmd in &self.execs {
            args.push(cmd.to_cmdline_lossy());
        }
        write!(f, "Pipeline {{ {} }}", args.join(" | "))
    }
}
