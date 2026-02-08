#[cfg(unix)]
mod os {
    pub const SHELL: [&str; 2] = ["sh", "-c"];
}

#[cfg(windows)]
mod os {
    pub const SHELL: [&str; 2] = ["cmd.exe", "/c"];
}

use std::borrow::Cow;
use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::ErrorKind;
use std::io::{self, Read, Write};
use std::ops::BitOr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::communicate::Communicator;
use crate::process::{ExitStatus, Process};

use crate::pipeline::Pipeline;
use os::*;

/// Instruction what to do with a stream in the child process.
///
/// `Redirection` values are used for the `stdin`, `stdout`, and `stderr` parameters when
/// configuring a subprocess via [`Exec`] or [`Pipeline`].
///
/// [`Exec`]: struct.Exec.html
/// [`Pipeline`]: struct.Pipeline.html
#[derive(Debug)]
pub enum Redirection {
    /// Do nothing with the stream.
    ///
    /// The stream is typically inherited from the parent. The corresponding pipe field in
    /// [`Job`] will be `None`.
    ///
    /// [`Job`]: struct.Job.html
    None,

    /// Redirect the stream to a pipe.
    ///
    /// This variant requests that a stream be redirected to a unidirectional pipe. One
    /// end of the pipe is passed to the child process and configured as one of its
    /// standard streams, and the other end is available to the parent for communicating
    /// with the child.
    Pipe,

    /// Merge the stream to the other output stream.
    ///
    /// This variant is only valid when configuring redirection of standard output and
    /// standard error. Using `Redirection::Merge` for stderr requests the child's stderr
    /// to refer to the same underlying file as the child's stdout (which may or may not
    /// itself be redirected), equivalent to the `2>&1` operator of the Bourne
    /// shell. Analogously, using `Redirection::Merge` for stdout is equivalent to `1>&2`
    /// in the shell.
    ///
    /// Specifying `Redirection::Merge` for stdin or specifying it for both stdout and
    /// stderr is invalid and will cause an error.
    Merge,

    /// Redirect the stream to the specified open `File`.
    ///
    /// This does not create a pipe, it simply spawns the child so that the specified
    /// stream sees that file. The child can read from or write to the provided file on
    /// its own, without any intervention by the parent.
    File(File),

    /// Redirect the stream to the null device (`/dev/null` on Unix, `nul` on Windows).
    ///
    /// This is equivalent to `Redirection::File` with a null device file, but more
    /// convenient and portable.
    Null,
}

/// A builder for creating subprocesses.
///
/// `Exec` provides a builder API for spawning subprocesses, and includes convenience
/// methods for capturing the output and for connecting subprocesses into pipelines.
///
/// # Examples
///
/// Execute an external command and wait for it to complete:
///
/// ```no_run
/// # use subprocess::*;
/// # fn dummy() -> std::io::Result<()> {
/// # let dirname = "some_dir";
/// let exit_status = Exec::cmd("umount").arg(dirname).join()?;
/// # Ok(())
/// # }
/// ```
///
/// Execute the command using the OS shell, like C's `system`:
///
/// ```no_run
/// # use subprocess::*;
/// # fn dummy() -> std::io::Result<()> {
/// Exec::shell("shutdown -h now").join()?;
/// # Ok(())
/// # }
/// ```
///
/// Start a subprocess and obtain its output as an `impl Read`, like C's `popen`:
///
/// ```
/// # use subprocess::*;
/// # fn dummy() -> std::io::Result<()> {
/// let stream = Exec::cmd("ls").stream_stdout()?;
/// // call stream.read_to_string, construct io::BufReader(stream), etc.
/// # Ok(())
/// # }
/// ```
///
/// Capture the output of a command:
///
/// ```
/// # use subprocess::*;
/// # fn dummy() -> std::io::Result<()> {
/// let out = Exec::cmd("ls").capture()?.stdout_str();
/// # Ok(())
/// # }
/// ```
///
/// Redirect standard error to standard output, and capture both in a single stream:
///
/// ```
/// # use subprocess::*;
/// # fn dummy() -> std::io::Result<()> {
/// let out_and_err = Exec::cmd("ls")
///   .stderr(Redirection::Merge)
///   .capture()?
///   .stdout_str();
/// # Ok(())
/// # }
/// ```
///
/// Provide input to the command and read its output:
///
/// ```
/// # use subprocess::*;
/// # fn dummy() -> std::io::Result<()> {
/// let out = Exec::cmd("sort")
///   .stdin("b\nc\na\n")
///   .capture()?
///   .stdout_str();
/// assert!(out == "a\nb\nc\n");
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
#[must_use]
pub struct Exec {
    command: OsString,
    args: Vec<OsString>,
    check_success: bool,
    stdin_data: Option<Vec<u8>>,
    pub(crate) stdin_redirect: Arc<Redirection>,
    pub(crate) stdout_redirect: Arc<Redirection>,
    pub(crate) stderr_redirect: Arc<Redirection>,
    detached: bool,
    executable: Option<OsString>,
    env: Option<Vec<(OsString, OsString)>>,
    cwd: Option<OsString>,
    #[cfg(unix)]
    setuid: Option<u32>,
    #[cfg(unix)]
    setgid: Option<u32>,
    #[cfg(unix)]
    setpgid: Option<u32>,
    #[cfg(windows)]
    creation_flags: u32,
}

impl Exec {
    /// Constructs a new `Exec`, configured to run `command`.
    ///
    /// The command will be run directly in the OS, without an intervening shell. To run
    /// it through a shell, use [`Exec::shell`] instead.
    ///
    /// By default, the command will be run without arguments, and none of the standard
    /// streams will be modified.
    ///
    /// [`Exec::shell`]: struct.Exec.html#method.shell
    pub fn cmd(command: impl AsRef<OsStr>) -> Exec {
        Exec {
            command: command.as_ref().to_owned(),
            args: vec![],
            check_success: false,
            stdin_data: None,
            stdin_redirect: Arc::new(Redirection::None),
            stdout_redirect: Arc::new(Redirection::None),
            stderr_redirect: Arc::new(Redirection::None),
            detached: false,
            executable: None,
            env: None,
            cwd: None,
            #[cfg(unix)]
            setuid: None,
            #[cfg(unix)]
            setgid: None,
            #[cfg(unix)]
            setpgid: None,
            #[cfg(windows)]
            creation_flags: 0,
        }
    }

    /// Constructs a new `Exec`, configured to run `cmdstr` with the system shell.
    ///
    /// `subprocess` never spawns shells without an explicit request. This command
    /// requests the shell to be used; on Unix-like systems, this is equivalent to
    /// `Exec::cmd("sh").arg("-c").arg(cmdstr)`. On Windows, it runs
    /// `Exec::cmd("cmd.exe").arg("/c")`.
    ///
    /// `shell` is useful for porting code that uses the C `system` function, which also
    /// spawns a shell.
    ///
    /// When invoking this function, be careful not to interpolate arguments into the
    /// string run by the shell, such as `Exec::shell(format!("sort {}", filename))`. Such
    /// code is prone to errors and, if `filename` comes from an untrusted source, to
    /// shell injection attacks. Instead, use `Exec::cmd("sort").arg(filename)`.
    pub fn shell(cmdstr: impl AsRef<OsStr>) -> Exec {
        Exec::cmd(SHELL[0]).args(&SHELL[1..]).arg(cmdstr)
    }

    /// Appends `arg` to argument list.
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Exec {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    /// Extends the argument list with `args`.
    pub fn args(mut self, args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Exec {
        self.args
            .extend(args.into_iter().map(|x| x.as_ref().to_owned()));
        self
    }

    /// Specifies that the process is initially detached.
    ///
    /// A detached process means that we will not wait for the process to finish when the
    /// object that owns it goes out of scope.
    pub fn detached(mut self) -> Exec {
        self.detached = true;
        self
    }

    /// If called, [`join`](Self::join) and [`capture`](Self::capture) will return an
    /// error if the process exits with a non-zero status.
    pub fn checked(mut self) -> Exec {
        self.check_success = true;
        self
    }

    fn ensure_env(&mut self) -> &mut Vec<(OsString, OsString)> {
        self.env.get_or_insert_with(|| env::vars_os().collect())
    }

    /// Clears the environment of the subprocess.
    ///
    /// When this is invoked, the subprocess will not inherit the environment of this
    /// process.
    pub fn env_clear(mut self) -> Exec {
        self.env = Some(vec![]);
        self
    }

    /// Sets an environment variable in the child process.
    ///
    /// If the same variable is set more than once, the last value is used.
    ///
    /// Other environment variables are by default inherited from the current process. If
    /// this is undesirable, call `env_clear` first.
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Exec {
        self.ensure_env()
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    /// Sets multiple environment variables in the child process.
    ///
    /// The keys and values of the variables are specified by the iterable.  If the same
    /// variable is set more than once, the last value is used.
    ///
    /// Other environment variables are by default inherited from the current process. If
    /// this is undesirable, call `env_clear` first.
    pub fn env_extend(
        mut self,
        vars: impl IntoIterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>,
    ) -> Exec {
        self.ensure_env().extend(
            vars.into_iter()
                .map(|(k, v)| (k.as_ref().to_owned(), v.as_ref().to_owned())),
        );
        self
    }

    /// Removes an environment variable from the child process.
    ///
    /// Other environment variables are inherited by default.
    pub fn env_remove(mut self, key: impl AsRef<OsStr>) -> Exec {
        self.ensure_env().retain(|(k, _v)| k != key.as_ref());
        self
    }

    /// Specifies the current working directory of the child process.
    ///
    /// If unspecified, the current working directory is inherited from the parent.
    pub fn cwd(mut self, dir: impl AsRef<Path>) -> Exec {
        self.cwd = Some(dir.as_ref().as_os_str().to_owned());
        self
    }

    /// Specifies how to set up the standard input of the child process.
    ///
    /// Argument can be:
    ///
    /// * a [`Redirection`];
    /// * a `File`, which is a shorthand for `Redirection::File(file)`;
    /// * a `Vec<u8>`, `&str`, or `&[u8]`, which will set up a `Redirection::Pipe` for
    ///   stdin, making sure that `capture` feeds that data into the standard input of the
    ///   subprocess.
    ///
    /// [`Redirection`]: enum.Redirection.html
    pub fn stdin(mut self, stdin: impl InputRedirection) -> Exec {
        match stdin.into_input_redirection() {
            InputRedirectionKind::AsRedirection(new) => {
                self.stdin_redirect = Arc::new(new);
                self.stdin_data = None;
            }
            InputRedirectionKind::FeedData(data) => {
                self.stdin_redirect = Arc::new(Redirection::Pipe);
                self.stdin_data = Some(data);
            }
        }
        self
    }

    /// Specifies how to set up the standard output of the child process.
    ///
    /// Argument can be:
    ///
    /// * a [`Redirection`];
    /// * a `File`, which is a shorthand for `Redirection::File(file)`.
    ///
    /// [`Redirection`]: enum.Redirection.html
    pub fn stdout(mut self, stdout: impl OutputRedirection) -> Exec {
        self.stdout_redirect = Arc::new(stdout.into_output_redirection());
        self
    }

    /// Specifies how to set up the standard error of the child process.
    ///
    /// Argument can be:
    ///
    /// * a [`Redirection`];
    /// * a `File`, which is a shorthand for `Redirection::File(file)`.
    ///
    /// [`Redirection`]: enum.Redirection.html
    pub fn stderr(mut self, stderr: impl OutputRedirection) -> Exec {
        self.stderr_redirect = Arc::new(stderr.into_output_redirection());
        self
    }

    fn check_no_stdin_data(&self, meth: &str) {
        if self.stdin_data.is_some() {
            panic!("{} called with input data specified", meth);
        }
    }

    // Terminators

    /// Spawn the process and return the raw spawn result.
    ///
    /// This is the low-level entry point used by both `start()` and
    /// `Pipeline::start()`. It calls `crate::spawn::spawn()` with the Exec's fields.
    pub(crate) fn spawn(self) -> io::Result<crate::spawn::SpawnResult> {
        let mut argv = self.args;
        argv.insert(0, self.command);

        crate::spawn::spawn(
            argv,
            self.stdin_redirect,
            self.stdout_redirect,
            self.stderr_redirect,
            self.detached,
            self.executable.as_deref(),
            self.env.as_deref(),
            self.cwd.as_deref(),
            #[cfg(unix)]
            self.setuid,
            #[cfg(unix)]
            self.setgid,
            #[cfg(unix)]
            self.setpgid,
            #[cfg(windows)]
            self.creation_flags,
        )
    }

    /// Starts the process and returns a `Job` handle with the running process and its
    /// pipe ends.
    pub fn start(mut self) -> io::Result<Job> {
        let stdin_data = self.stdin_data.take().unwrap_or_default();
        let check_success = self.check_success;

        let result = self.spawn()?;

        Ok(Job {
            stdin: result.stdin,
            stdout: result.stdout,
            stderr: result.stderr,
            stdin_data,
            check_success,
            processes: vec![result.process],
        })
    }

    /// Starts the process, waits for it to finish, and returns the exit status.
    pub fn join(self) -> io::Result<ExitStatus> {
        self.start()?.join()
    }

    /// Starts the process and returns a value implementing the `Read` trait that reads
    /// from the standard output of the child process.
    ///
    /// This will automatically set up `stdout(Redirection::Pipe)`, so it is not necessary
    /// to do that beforehand.
    ///
    /// When the trait object is dropped, it will wait for the process to finish. If this
    /// is undesirable, use `detached()`.
    ///
    /// # Panics
    ///
    /// Panics if input data was specified with [`stdin`](Self::stdin). Use
    /// [`capture`](Self::capture) or [`communicate`](Self::communicate) to both feed
    /// input and read output.
    pub fn stream_stdout(self) -> io::Result<impl Read> {
        self.check_no_stdin_data("stream_stdout");
        Ok(ReadAdapter(self.stdout(Redirection::Pipe).start()?))
    }

    /// Starts the process and returns a value implementing the `Read` trait that reads
    /// from the standard error of the child process.
    ///
    /// This will automatically set up `stderr(Redirection::Pipe)`, so it is not necessary
    /// to do that beforehand.
    ///
    /// When the trait object is dropped, it will wait for the process to finish. If this
    /// is undesirable, use `detached()`.
    ///
    /// # Panics
    ///
    /// Panics if input data was specified with [`stdin`](Self::stdin). Use
    /// [`capture`](Self::capture) or [`communicate`](Self::communicate) to both feed
    /// input and read output.
    pub fn stream_stderr(self) -> io::Result<impl Read> {
        self.check_no_stdin_data("stream_stderr");
        Ok(ReadErrAdapter(self.stderr(Redirection::Pipe).start()?))
    }

    /// Starts the process and returns a value implementing the `Write` trait that writes
    /// to the standard input of the child process.
    ///
    /// This will automatically set up `stdin(Redirection::Pipe)`, so it is not necessary
    /// to do that beforehand.
    ///
    /// When the trait object is dropped, it will wait for the process to finish. If this
    /// is undesirable, use `detached()`.
    ///
    /// # Panics
    ///
    /// Panics if input data was specified with [`stdin`](Self::stdin).
    pub fn stream_stdin(self) -> io::Result<impl Write> {
        self.check_no_stdin_data("stream_stdin");
        Ok(WriteAdapter(self.stdin(Redirection::Pipe).start()?))
    }

    /// Starts the process and returns a `Communicator` handle.
    ///
    /// Unless already configured, stdout and stderr are redirected to pipes. To only
    /// communicate over specific streams, set them up explicitly and use `start()`.
    ///
    /// Compared to `capture()`, this offers more choice in how communication is
    /// performed, such as read size limit and timeout.
    ///
    /// Unlike `capture()`, this method doesn't wait for the process to finish,
    /// effectively detaching it.
    pub fn communicate(mut self) -> io::Result<Communicator<Vec<u8>>> {
        self = self.detached();
        if matches!(*self.stdout_redirect, Redirection::None) {
            self = self.stdout(Redirection::Pipe);
        }
        if matches!(*self.stderr_redirect, Redirection::None) {
            self = self.stderr(Redirection::Pipe);
        }
        Ok(self.start()?.communicate())
    }

    /// Starts the process, collects its output, and waits for it to finish.
    ///
    /// The return value provides the standard output and standard error as bytes or
    /// optionally strings, as well as the exit status.
    ///
    /// Unless already configured, stdout and stderr are redirected to pipes so they can
    /// be captured. To only capture stdout, set it up explicitly and use `start()`:
    ///
    /// ```ignore
    /// let c = Exec::cmd("foo").stdout(Redirection::Pipe).start()?.capture()?;
    /// ```
    ///
    /// This method waits for the process to finish, rather than simply waiting for its
    /// standard streams to close. If this is undesirable, use `detached()`.
    pub fn capture(mut self) -> io::Result<Capture> {
        if matches!(*self.stdout_redirect, Redirection::None) {
            self = self.stdout(Redirection::Pipe);
        }
        if matches!(*self.stderr_redirect, Redirection::None) {
            self = self.stderr(Redirection::Pipe);
        }
        self.start()?.capture()
    }

    // used for Debug impl
    pub(crate) fn display_escape(s: &str) -> Cow<'_, str> {
        fn nice_char(c: char) -> bool {
            match c {
                '-' | '_' | '.' | ',' | '/' => true,
                c if c.is_ascii_alphanumeric() => true,
                _ => false,
            }
        }
        if !s.chars().all(nice_char) {
            Cow::Owned(format!("'{}'", s.replace("'", r#"'\''"#)))
        } else {
            Cow::Borrowed(s)
        }
    }

    /// Show Exec as command-line string quoted in the Unix style.
    pub fn to_cmdline_lossy(&self) -> String {
        let mut out = String::new();
        if let Some(cmd_env) = &self.env {
            let current: Vec<_> = env::vars_os().collect();
            let current_map: HashMap<_, _> = current.iter().map(|(x, y)| (x, y)).collect();
            for (k, v) in cmd_env {
                if current_map.get(k) == Some(&v) {
                    continue;
                }
                out.push_str(&Exec::display_escape(&k.to_string_lossy()));
                out.push('=');
                out.push_str(&Exec::display_escape(&v.to_string_lossy()));
                out.push(' ');
            }
            let cmd_env: HashMap<_, _> = cmd_env.iter().map(|(k, v)| (k, v)).collect();
            for (k, _) in current {
                if !cmd_env.contains_key(&k) {
                    out.push_str(&Exec::display_escape(&k.to_string_lossy()));
                    out.push('=');
                    out.push(' ');
                }
            }
        }
        out.push_str(&Exec::display_escape(&self.command.to_string_lossy()));
        for arg in &self.args {
            out.push(' ');
            out.push_str(&Exec::display_escape(&arg.to_string_lossy()));
        }
        out
    }

    pub(crate) fn stdin_is_set(&self) -> bool {
        !matches!(*self.stdin_redirect, Redirection::None)
    }

    pub(crate) fn stdout_is_set(&self) -> bool {
        !matches!(*self.stdout_redirect, Redirection::None)
    }

    #[cfg(unix)]
    pub(crate) fn setpgid_is_set(&self) -> bool {
        self.setpgid.is_some()
    }

    #[cfg(unix)]
    pub(crate) fn set_pgid_value(&mut self, pgid: u32) {
        self.setpgid = Some(pgid);
    }
}

impl BitOr for Exec {
    type Output = Pipeline;

    /// Create a `Pipeline` from `self` and `rhs`.
    fn bitor(self, rhs: Exec) -> Pipeline {
        Pipeline::new().pipe(self).pipe(rhs)
    }
}

impl fmt::Debug for Exec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Exec {{ {} }}", self.to_cmdline_lossy())
    }
}

/// A started process or pipeline, consisting of running processes and their pipe ends.
///
/// Created by [`Exec::start`] or [`Pipeline::start`].
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
    pub stdin_data: Vec<u8>,
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
    pub fn communicate(&mut self) -> Communicator<Vec<u8>> {
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
    /// Unix and calls `TerminateProcess` on Windows.
    pub fn terminate(&self) -> io::Result<()> {
        for p in &self.processes {
            p.terminate()?;
        }
        Ok(())
    }

    /// Waits for all processes to finish and returns the last process's exit status.
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

    /// Kill all processes in the pipeline.
    ///
    /// Delegates to [`Process::kill()`] on each process, which sends `SIGKILL` on Unix
    /// and calls `TerminateProcess` on Windows.
    pub fn kill(&self) -> io::Result<()> {
        for p in &self.processes {
            p.kill()?;
        }
        Ok(())
    }

    /// Poll all processes for completion without blocking.
    ///
    /// Returns `Some(exit_status)` of the last process if all processes have finished, or
    /// `None` if any process is still running.
    pub fn poll(&self) -> Option<ExitStatus> {
        let mut status = None;
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
        self.communicate().read()?;
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
        self.communicate().limit_time(timeout).read()?;
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
        let mut comm = self.communicate();
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
        let mut comm = self.communicate().limit_time(timeout);
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

/// Data captured by [`Exec::capture`] and [`Pipeline::capture`].
///
/// [`Exec::capture`]: struct.Exec.html#method.capture
/// [`Pipeline::capture`]: struct.Pipeline.html#method.capture
#[derive(Debug)]
pub struct Capture {
    /// Standard output as bytes.
    pub stdout: Vec<u8>,
    /// Standard error as bytes.
    pub stderr: Vec<u8>,
    /// Exit status.
    pub exit_status: ExitStatus,
}

impl Capture {
    /// Returns the standard output as string, converted from bytes using
    /// `String::from_utf8_lossy`.
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Returns the standard error as string, converted from bytes using
    /// `String::from_utf8_lossy`.
    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// True if the exit status of the process or pipeline is 0.
    pub fn success(&self) -> bool {
        self.exit_status.success()
    }

    /// Returns `self` if the exit status is successful, or an error otherwise.
    pub fn check(self) -> io::Result<Self> {
        if self.success() {
            Ok(self)
        } else {
            Err(io::Error::other(format!(
                "command failed: {}",
                self.exit_status
            )))
        }
    }
}

#[derive(Debug)]
pub enum InputRedirectionKind {
    AsRedirection(Redirection),
    FeedData(Vec<u8>),
}

mod sealed {
    pub trait InputRedirectionSealed {}
    pub trait OutputRedirectionSealed {}
}

/// Trait for types that can be used to redirect standard input.
///
/// This is a sealed trait that cannot be implemented outside this crate.
#[allow(private_interfaces)]
pub trait InputRedirection: sealed::InputRedirectionSealed {
    /// Convert to internal representation.
    #[doc(hidden)]
    fn into_input_redirection(self) -> InputRedirectionKind;
}

/// Trait for types that can be used to redirect standard output or standard error.
///
/// This is a sealed trait that cannot be implemented outside this crate.
pub trait OutputRedirection: sealed::OutputRedirectionSealed {
    /// Convert to internal representation.
    #[doc(hidden)]
    fn into_output_redirection(self) -> Redirection;
}

impl sealed::InputRedirectionSealed for Redirection {}
impl InputRedirection for Redirection {
    fn into_input_redirection(self) -> InputRedirectionKind {
        if let Redirection::Merge = self {
            panic!("Redirection::Merge is only allowed for output streams");
        }
        InputRedirectionKind::AsRedirection(self)
    }
}

impl sealed::InputRedirectionSealed for File {}
impl InputRedirection for File {
    fn into_input_redirection(self) -> InputRedirectionKind {
        InputRedirectionKind::AsRedirection(Redirection::File(self))
    }
}

impl sealed::InputRedirectionSealed for Vec<u8> {}
impl InputRedirection for Vec<u8> {
    fn into_input_redirection(self) -> InputRedirectionKind {
        InputRedirectionKind::FeedData(self)
    }
}

impl sealed::InputRedirectionSealed for &str {}
impl InputRedirection for &str {
    fn into_input_redirection(self) -> InputRedirectionKind {
        InputRedirectionKind::FeedData(self.as_bytes().to_vec())
    }
}

impl sealed::InputRedirectionSealed for &[u8] {}
impl InputRedirection for &[u8] {
    fn into_input_redirection(self) -> InputRedirectionKind {
        InputRedirectionKind::FeedData(self.to_vec())
    }
}

impl<const N: usize> sealed::InputRedirectionSealed for &[u8; N] {}
impl<const N: usize> InputRedirection for &[u8; N] {
    fn into_input_redirection(self) -> InputRedirectionKind {
        InputRedirectionKind::FeedData(self.to_vec())
    }
}

impl sealed::OutputRedirectionSealed for Redirection {}
impl OutputRedirection for Redirection {
    fn into_output_redirection(self) -> Redirection {
        self
    }
}

impl sealed::OutputRedirectionSealed for File {}
impl OutputRedirection for File {
    fn into_output_redirection(self) -> Redirection {
        Redirection::File(self)
    }
}

#[cfg(unix)]
pub mod unix {
    use super::{Exec, Job};
    use crate::pipeline::Pipeline;
    use crate::unix::ProcessExt;
    use std::io;

    /// Unix-specific extension methods for [`Job`].
    pub trait JobExt {
        /// Send the specified signal to all processes in the pipeline.
        ///
        /// Delegates to [`ProcessExt::send_signal`] on each process.
        fn send_signal(&self, signal: i32) -> io::Result<()>;

        /// Send the specified signal to the process group of the first process.
        ///
        /// When used with [`PipelineExt::setpgid`], all pipeline processes share the
        /// first process's group, so signaling it reaches the entire pipeline. For a
        /// single process started with [`ExecExt::setpgid`], this signals its group.
        fn send_signal_group(&self, signal: i32) -> io::Result<()>;
    }

    impl JobExt for Job {
        fn send_signal(&self, signal: i32) -> io::Result<()> {
            for p in &self.processes {
                p.send_signal(signal)?;
            }
            Ok(())
        }

        fn send_signal_group(&self, signal: i32) -> io::Result<()> {
            if let Some(p) = self.processes.first() {
                p.send_signal_group(signal)?;
            }
            Ok(())
        }
    }

    /// Extension trait for Unix-specific process creation options.
    pub trait ExecExt {
        /// Set the user ID for the spawned process.
        ///
        /// The child process will run with the specified user ID, which affects file
        /// access permissions and process ownership. This calls `setuid(2)` in the child
        /// process after `fork()` but before `exec()`.
        fn setuid(self, uid: u32) -> Self;

        /// Set the group ID for the spawned process.
        ///
        /// The child process will run with the specified group ID, which affects file
        /// access permissions based on group ownership. This calls `setgid(2)` in the
        /// child process after `fork()` but before `exec()`.
        fn setgid(self, gid: u32) -> Self;

        /// Put the subprocess into its own process group.
        ///
        /// This calls `setpgid(0, 0)` before execing the child process, making it the
        /// leader of a new process group.  Useful for a single process that spawns
        /// children, allowing them all to be signaled as a group with
        /// [`ProcessExt::send_signal_group`].
        ///
        /// For pipelines, use [`PipelineExt::setpgid`] instead, which puts all pipeline
        /// processes into a shared group.
        ///
        /// [`ProcessExt::send_signal_group`]: crate::unix::ProcessExt::send_signal_group
        /// [`PipelineExt::setpgid`]: PipelineExt::setpgid
        fn setpgid(self) -> Self;
    }

    impl ExecExt for Exec {
        fn setuid(mut self, uid: u32) -> Exec {
            self.setuid = Some(uid);
            self
        }

        fn setgid(mut self, gid: u32) -> Exec {
            self.setgid = Some(gid);
            self
        }

        fn setpgid(mut self) -> Exec {
            self.setpgid = Some(0);
            self
        }
    }

    /// Unix-specific extension methods for [`Pipeline`].
    pub trait PipelineExt {
        /// Put all pipeline processes into a shared process group.
        ///
        /// The first process becomes the group leader (via `setpgid(0, 0)`) and
        /// subsequent processes join its group.  This allows signaling the entire
        /// pipeline as a unit using [`JobExt::send_signal_group`].
        ///
        /// For single processes that spawn children, use [`ExecExt::setpgid`] instead.
        fn setpgid(self) -> Self;
    }

    impl PipelineExt for Pipeline {
        fn setpgid(mut self) -> Pipeline {
            self.set_setpgid(true);
            self
        }
    }
}

#[cfg(windows)]
pub mod windows {
    use super::Exec;

    /// Process creation flag: The process does not have a console window.
    pub const CREATE_NO_WINDOW: u32 = 0x08000000;

    /// Process creation flag: The new process has a new console.
    pub const CREATE_NEW_CONSOLE: u32 = 0x00000010;

    /// Process creation flag: The new process is the root of a new process
    /// group.
    pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;

    /// Process creation flag: The process does not inherit its parent's
    /// console.
    pub const DETACHED_PROCESS: u32 = 0x00000008;

    /// Extension trait for Windows-specific process creation options.
    pub trait ExecExt {
        /// Set process creation flags for Windows.
        ///
        /// This value is passed to the `dwCreationFlags` parameter of
        /// `CreateProcessW`. Use this to control process creation behavior
        /// such as creating the process without a console window.
        fn creation_flags(self, flags: u32) -> Self;
    }

    impl ExecExt for Exec {
        fn creation_flags(mut self, flags: u32) -> Exec {
            self.creation_flags = flags;
            self
        }
    }
}
