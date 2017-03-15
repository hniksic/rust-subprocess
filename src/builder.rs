#[cfg(unix)]
mod os {
    pub const NULL_DEVICE: &'static str = "/dev/null";
    pub const SHELL: [&'static str; 2] = ["sh", "-c"];
}

#[cfg(windows)]
mod os {
    pub const NULL_DEVICE: &'static str = "nul";
    pub const SHELL: [&'static str; 2] = ["cmd.exe", "/c"];
}

pub use self::os::*;
pub use self::exec::{Exec, NullFile};
pub use self::pipeline::Pipeline;


mod exec {
    use std::ffi::{OsStr, OsString};
    use std::io::{Result as IoResult, Read, Write};
    use std::fs::{File, OpenOptions};
    use std::ops::BitOr;

    use popen::{PopenConfig, Popen, Redirection, Result as PopenResult};
    use os_common::ExitStatus;

    use super::os::*;
    use super::Pipeline;

    /// A builder for [`Popen`] instances, providing control and
    /// convenience methods.
    ///
    /// `Exec` provides a builder API for [`Popen::create`], and
    /// includes convenience methods for capturing the output, and for
    /// connecting subprocesses into pipelines.
    ///
    /// # Examples
    ///
    /// Execute an external command and wait for it to complete:
    ///
    /// ```no_run
    /// # use subprocess::*;
    /// # fn dummy() -> Result<()> {
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
    /// # fn dummy() -> Result<()> {
    /// Exec::shell("shutdown -h now").join()?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Start a subprocess and obtain its output as a `Read` trait object,
    /// like C's `popen`:
    ///
    /// ```
    /// # use subprocess::*;
    /// # fn dummy() -> Result<()> {
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
    /// # fn dummy() -> Result<()> {
    /// let out = Exec::cmd("ls")
    ///   .stdout(Redirection::Pipe)
    ///   .capture()?
    ///   .stdout_str();
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Redirect errors to standard output, and capture both in a single stream:
    ///
    /// ```
    /// # use subprocess::*;
    /// # fn dummy() -> Result<()> {
    /// let out_and_err = Exec::cmd("ls")
    ///   .stdout(Redirection::Pipe)
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
    /// # fn dummy() -> Result<()> {
    /// let out = Exec::cmd("sort")
    ///   .stdin("b\nc\na\n")
    ///   .stdout(Redirection::Pipe)
    ///   .capture()?
    ///   .stdout_str();
    /// assert!(out == "a\nb\nc\n");
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// [`Popen`]: struct.Popen.html
    /// [`Popen::create`]: struct.Popen.html#method.create

    #[derive(Debug)]
    pub struct Exec {
        command: OsString,
        args: Vec<OsString>,
        config: PopenConfig,
        stdin_data: Option<Vec<u8>>,
    }

    impl Exec {
        /// Constructs a new `Exec`, configured to run `command`.
        ///
        /// The command will be run directly in the OS, without an
        /// intervening shell.  To run it through a shell, use
        /// [`Exec::shell`] instead.
        ///
        /// By default, the command will be run without arguments, and
        /// none of the standard streams will be modified.
        ///
        /// [`Exec::shell`]: struct.Exec.html#method.shell
        pub fn cmd<S: AsRef<OsStr>>(command: S) -> Exec {
            Exec {
                command: command.as_ref().to_owned(),
                args: vec![],
                config: PopenConfig::default(),
                stdin_data: None,
            }
        }

        /// Constructs a new `Exec`, configured to run `cmdstr` with
        /// the system shell.
        ///
        /// `subprocess` never spawns shells without an explicit
        /// request.  This command requests the shell to be used; on
        /// Unix-like systems, this is equivalent to
        /// `Exec::cmd("sh").arg("-c").arg(cmdstr)`.  On Windows, it
        /// runs `Exec::cmd("cmd.exe").arg("/c")`.
        ///
        /// `shell` is useful for porting code that uses the C
        /// `system` function, which also spawns a shell.
        ///
        /// When invoking this function, be careful not to interpolate
        /// arguments into the string run by the shell, such as
        /// `Exec::shell(format!("sort {}", filename))`.  Such code is
        /// prone to errors and, if `filename` comes from an untrusted
        /// source, to shell injection attacks.  Instead, use
        /// `Exec::cmd("sort").arg(filename)`.
        pub fn shell<S: AsRef<OsStr>>(cmdstr: S) -> Exec {
            Exec::cmd(SHELL[0]).args(&SHELL[1..]).arg(cmdstr)
        }

        /// Appends `arg` to argument list.
        pub fn arg<S: AsRef<OsStr>>(mut self, arg: S) -> Exec {
            self.args.push(arg.as_ref().to_owned());
            self
        }

        /// Extends the argument list with `args`.
        pub fn args<S: AsRef<OsStr>>(mut self, args: &[S]) -> Exec {
            self.args.extend(args.iter().map(|x| x.as_ref().to_owned()));
            self
        }

        /// Specifies that the process is initially detached.
        ///
        /// A detached process means that we will not wait for the
        /// process to finish when the object that owns it goes out of
        /// scope.
        pub fn detached(mut self) -> Exec {
            self.config.detached = true;
            self
        }

        fn ensure_env(&mut self) {
            if self.config.env.is_none() {
                self.config.env = Some(PopenConfig::current_env());
            }
        }

        /// Clear the environment of the subprocess.
        ///
        /// When this is invoked, the subprocess will not inherit the
        /// environment of this process.
        pub fn env_clear(mut self) -> Exec {
            self.config.env = Some(Vec::new());
            self
        }

        /// Specifies the value of an environment variable in the child process.
        ///
        /// Other environment variables are inherited by default.  If
        /// this is undesirable, call `env_clear` first.
        pub fn env<K, V>(mut self, key: K, value: V) -> Exec
            where K: AsRef<OsStr>,
                  V: AsRef<OsStr>
        {
            self.ensure_env();
            self.config.env.as_mut().unwrap().push((key.as_ref().to_owned(),
                                                    value.as_ref().to_owned()));
            self
        }

        /// Removes an environment variable from the child process.
        ///
        /// Other environment variables are inherited by default.
        pub fn env_remove<K>(mut self, key: K) -> Exec
            where K: AsRef<OsStr>
        {
            self.ensure_env();
            self.config.env.as_mut().unwrap().retain(
                |&(ref k, ref _v)| k != key.as_ref());
            self
        }

        /// Specifies how to set up the standard input of the child process.
        ///
        /// Argument can be:
        ///
        /// * a [`Redirection`];
        /// * a `File`, which is a shorthand for `Redirection::File(file)`;
        /// * a `Vec<u8>` or `&str`, which will set up a `Redirection::Pipe`
        ///   for stdin, making sure that `capture` feeds that data into the
        ///   standard input of the subprocess;
        /// * [`NullFile`], which will redirect the standard input to read from
        ///    `/dev/null`.
        ///
        /// [`Redirection`]: struct.Redirection.html
        /// [`NullFile`]: struct.NullFile.html
        pub fn stdin<T: IntoInputRedirection>(mut self, stdin: T) -> Exec {
            match (&self.config.stdin, stdin.into_input_redirection()) {
                (&Redirection::None, InputRedirection::AsRedirection(new))
                    => self.config.stdin = new,
                (&Redirection::Pipe,
                 InputRedirection::AsRedirection(Redirection::Pipe)) => (),
                (&Redirection::None, InputRedirection::FeedData(data)) => {
                    self.config.stdin = Redirection::Pipe;
                    self.stdin_data = Some(data);
                }
                (_, _) => panic!("stdin is already set"),
            }
            self
        }

        /// Specifies how to set up the standard output of the child process.
        ///
        /// Argument can be:
        ///
        /// * a [`Redirection`];
        /// * a `File`, which is a shorthand for `Redirection::File(file)`;
        /// * [`NullFile`], which will redirect the standard output to go to
        ///    `/dev/null`.
        ///
        /// [`Redirection`]: struct.Redirection.html
        /// [`NullFile`]: struct.NullFile.html
        pub fn stdout<T: IntoOutputRedirection>(mut self, stdout: T) -> Exec {
            match (&self.config.stdout, stdout.into_output_redirection()) {
                (&Redirection::None, new) => self.config.stdout = new,
                (&Redirection::Pipe, Redirection::Pipe) => (),
                (_, _) => panic!("stdout is already set"),
            }
            self
        }

        /// Specifies how to set up the standard error of the child process.
        ///
        /// Argument can be:
        ///
        /// * a [`Redirection`];
        /// * a `File`, which is a shorthand for `Redirection::File(file)`;
        /// * [`NullFile`], which will redirect the standard error to go to
        ///    `/dev/null`.
        ///
        /// [`Redirection`]: struct.Redirection.html
        /// [`NullFile`]: struct.NullFile.html
        pub fn stderr<T: IntoOutputRedirection>(mut self, stderr: T) -> Exec {
            match (&self.config.stderr, stderr.into_output_redirection()) {
                (&Redirection::None, new) => self.config.stderr = new,
                (&Redirection::Pipe, Redirection::Pipe) => (),
                (_, _) => panic!("stderr is already set"),
            }
            self
        }

        fn check_no_stdin_data(&self, meth: &str) {
            if self.stdin_data.is_some() {
                panic!("{} called with input data specified", meth);
            }
        }

        // Terminators

        /// Starts the process, returning a `Popen` for the running process.
        pub fn popen(mut self) -> PopenResult<Popen> {
            self.check_no_stdin_data("popen");
            self.args.insert(0, self.command);
            let p = Popen::create(&self.args, self.config)?;
            Ok(p)
        }

        /// Starts the process, waits for it to finish, and returns
        /// the exit status.
        ///
        /// This method will wait for as long as necessary for the
        /// process to finish.  If a timeout is needed, use
        /// `popen()?.wait_timeout(...)` instead.
        pub fn join(self) -> PopenResult<ExitStatus> {
            self.check_no_stdin_data("join");
            self.popen()?.wait()
        }

        /// Starts the process and returns a `Read` trait object that
        /// reads from the standard output of the child process.
        ///
        /// This will automatically set up
        /// `stdout(Redirection::Pipe)`, so it is not necessary to do
        /// that beforehand.
        ///
        /// When the trait object is dropped, it will wait for the
        /// process to finish.  If this is undesirable, use
        /// `detached()`.
        pub fn stream_stdout(self) -> PopenResult<Box<Read>> {
            self.check_no_stdin_data("stream_stdout");
            let p = self.stdout(Redirection::Pipe).popen()?;
            Ok(Box::new(ReadOutAdapter(p)))
        }

        /// Starts the process and returns a `Read` trait object that
        /// reads from the standard error of the child process.
        ///
        /// This will automatically set up
        /// `stderr(Redirection::Pipe)`, so it is not necessary to do
        /// that beforehand.
        ///
        /// When the trait object is dropped, it will wait for the
        /// process to finish.  If this is undesirable, use
        /// `detached()`.
        pub fn stream_stderr(self) -> PopenResult<Box<Read>> {
            self.check_no_stdin_data("stream_stderr");
            let p = self.stderr(Redirection::Pipe).popen()?;
            Ok(Box::new(ReadErrAdapter(p)))
        }

        /// Starts the process and returns a `Write` trait object that
        /// writes to the standard input of the child process.
        ///
        /// This will automatically set up `stdin(Redirection::Pipe)`,
        /// so it is not necessary to do that beforehand.
        ///
        /// When the trait object is dropped, it will wait for the
        /// process to finish.  If this is undesirable, use
        /// `detached()`.
        pub fn stream_stdin(self) -> PopenResult<Box<Write>> {
            self.check_no_stdin_data("stream_stdin");
            let p = self.stdin(Redirection::Pipe).popen()?;
            Ok(Box::new(WriteAdapter(p)))
        }

        /// Starts the process, collects its output, and waits for it
        /// to finish.
        ///
        /// The return value provides the standard output and standard
        /// error as bytes or optionally strings, as well as the exit
        /// status.
        ///
        /// Unlike `Popen::communicate`, this method actually waits
        /// for the process to finish, rather than simply waiting for
        /// its standard streams to close.  If this is undesirable,
        /// use `detached()`.
        pub fn capture(mut self) -> PopenResult<Capture> {
            let stdin_data = self.stdin_data.take();
            if let (&Redirection::None, &Redirection::None)
                = (&self.config.stdout, &self.config.stderr) {
                self = self.stdout(Redirection::Pipe);
            }
            let mut p = self.popen()?;
            let (maybe_out, maybe_err) = p.communicate_bytes(
                stdin_data.as_ref().map(|v| &v[..]))?;
            let out = maybe_out.unwrap_or_else(Vec::new);
            let err = maybe_err.unwrap_or_else(Vec::new);
            let status = p.wait()?;
            Ok(Capture {
                stdout: out, stderr: err, exit_status: status
            })
        }
    }

    impl Clone for Exec {
        /// Returns a copy of the value.
        ///
        /// This method is guaranteed not to fail as long as none of
        /// the `Redirection` values contain a `Redirection::File`
        /// variant.  If a redirection to `File` is present, cloning
        /// that field will use `File::try_clone` method, which
        /// duplicates a file descriptor and can (but is not likely
        /// to) fail.  In that scenario, `Exec::clone` panics.
        fn clone(&self) -> Exec {
            Exec {
                command: self.command.clone(),
                args: self.args.clone(),
                config: self.config.try_clone().unwrap(),
                stdin_data: self.stdin_data.as_ref().cloned(),
            }
        }
    }

    impl BitOr for Exec {
        type Output = Pipeline;

        /// Create a `Pipeline` from `self` and `rhs`.
        fn bitor(self, rhs: Exec) -> Pipeline {
            Pipeline::new(self, rhs)
        }
    }

    #[derive(Debug)]
    struct ReadOutAdapter(Popen);

    impl Read for ReadOutAdapter {
        fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
            self.0.stdout.as_mut().unwrap().read(buf)
        }
    }

    #[derive(Debug)]
    struct ReadErrAdapter(Popen);

    impl Read for ReadErrAdapter {
        fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
            self.0.stderr.as_mut().unwrap().read(buf)
        }
    }

    #[derive(Debug)]
    struct WriteAdapter(Popen);

    impl Write for WriteAdapter {
        fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
            self.0.stdin.as_mut().unwrap().write(buf)
        }
        fn flush(&mut self) -> IoResult<()> {
            self.0.stdin.as_mut().unwrap().flush()
        }
    }

    // We must implement Drop in order to close the stream.  The typical
    // use case for stream_stdin() is a process that reads something from
    // stdin.  WriteAdapter going out of scope invokes Popen::drop(),
    // which waits for the process to exit.  Without closing stdin, this
    // deadlocks because the child process hangs reading its stdin.

    impl Drop for WriteAdapter {
        fn drop(&mut self) {
            self.0.stdin.take();
        }
    }

    pub struct Capture {
        /// Standard output as bytes.
        pub stdout: Vec<u8>,
        /// Standard error as bytes.
        pub stderr: Vec<u8>,
        /// Exit status of the process.
        pub exit_status: ExitStatus,
    }

    impl Capture {
        /// Returns the process's standard output as string, converted
        /// from bytes using `String::from_utf8_lossy`.
        pub fn stdout_str(&self) -> String {
            String::from_utf8_lossy(&self.stdout).into_owned()
        }

        /// Returns the process's standard error as string, converted
        /// from bytes using `String::from_utf8_lossy`.
        pub fn stderr_str(&self) -> String {
            String::from_utf8_lossy(&self.stderr).into_owned()
        }
    }

    pub enum InputRedirection {
        AsRedirection(Redirection),
        FeedData(Vec<u8>),
    }

    pub trait IntoInputRedirection {
        fn into_input_redirection(self) -> InputRedirection;
    }

    impl IntoInputRedirection for Redirection {
        fn into_input_redirection(self) -> InputRedirection {
            if let Redirection::Merge = self {
                panic!("Redirection::Merge is only allowed for output streams");
            }
            InputRedirection::AsRedirection(self)
        }
    }

    impl IntoInputRedirection for File {
        fn into_input_redirection(self) -> InputRedirection {
            InputRedirection::AsRedirection(Redirection::File(self))
        }
    }

    /// Marker value for [`stdin`], [`stdout`], and [`stderr`] methods
    /// of [`Exec`] and [`Pipeline`].
    ///
    /// Use of this value means that the corresponding stream should
    /// be redirected to the devnull device.
    ///
    /// [`stdin`]: struct.Exec.html#method.stdin
    /// [`stdout`]: struct.Exec.html#method.stdout
    /// [`stderr`]: struct.Exec.html#method.stderr
    /// [`Exec`]: struct.Exec.html
    /// [`Pipeline`]: struct.Pipeline.html
    pub struct NullFile;

    impl IntoInputRedirection for NullFile {
        fn into_input_redirection(self) -> InputRedirection {
            let null_file = OpenOptions::new().read(true)
                .open(NULL_DEVICE).unwrap();
            InputRedirection::AsRedirection(Redirection::File(null_file))
        }
    }

    impl IntoInputRedirection for Vec<u8> {
        fn into_input_redirection(self) -> InputRedirection {
            InputRedirection::FeedData(self)
        }
    }

    impl<'a> IntoInputRedirection for &'a str {
        fn into_input_redirection(self) -> InputRedirection {
            InputRedirection::FeedData(self.as_bytes().to_vec())
        }
    }

    pub trait IntoOutputRedirection {
        fn into_output_redirection(self) -> Redirection;
    }

    impl IntoOutputRedirection for Redirection {
        fn into_output_redirection(self) -> Redirection {
            self
        }
    }

    impl IntoOutputRedirection for File {
        fn into_output_redirection(self) -> Redirection {
            Redirection::File(self)
        }
    }

    impl IntoOutputRedirection for NullFile {
        fn into_output_redirection(self) -> Redirection {
            let null_file = OpenOptions::new().write(true)
                .open(NULL_DEVICE).unwrap();
            Redirection::File(null_file)
        }
    }
}


mod pipeline {
    use std::io::{Result as IoResult, Read, Write};
    use std::ops::BitOr;
    use std::fs::File;

    use popen::{Popen, Redirection, Result as PopenResult};
    use communicate;
    use os_common::ExitStatus;

    use super::exec::{Exec, IntoInputRedirection, InputRedirection,
                      IntoOutputRedirection};

    /// A builder for multiple [`Popen`] instances connected via
    /// pipes.
    ///
    /// A pipeline is a sequence of two or more [`Exec`] commands
    /// connected via pipes.  Just like in a Unix shell pipeline, each
    /// command receives standard input from the previous command, and
    /// passes standard output to the next command.  Optionally, the
    /// standard input of the first command can be provided from the
    /// outside, and the output of the last command can be captured.
    ///
    /// In most cases you do not need to create [`Pipeline`] instances
    /// directly; instead, combine [`Exec`] instances using the `|`
    /// operator which produces `Pipeline`.
    ///
    /// # Examples
    ///
    /// Execite a pipeline and return the exit status of the last command:
    ///
    /// ```no_run
    /// # use subprocess::*;
    /// # fn dummy() -> Result<()> {
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
    /// # fn dummy() -> Result<()> {
    /// let dir_checksum = {
    ///     Exec::cmd("find . -type f") | Exec::cmd("sort") | Exec::cmd("sha1sum")
    /// }.capture()?.stdout_str();
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// [`Popen`]: struct.Popen.html
    /// [`Exec`]: struct.Exec.html
    /// [`Pipeline`]: struct.Pipeline.html
    #[derive(Debug)]
    pub struct Pipeline {
        cmds: Vec<Exec>,
        stdin: Redirection,
        stdout: Redirection,
        stdin_data: Option<Vec<u8>>,
    }

    impl Pipeline {
        /// Creates a new pipeline by combining two commands.
        ///
        /// Equivalent to `cmd1 | cmd2`.
        pub fn new(cmd1: Exec, cmd2: Exec) -> Pipeline {
            Pipeline {
                cmds: vec![cmd1, cmd2],
                stdin: Redirection::None,
                stdout: Redirection::None,
                stdin_data: None,
            }
        }

        /// Specifies how to set up the standard input of the first
        /// command in the pipeline.
        ///
        /// Argument can be:
        ///
        /// * a [`Redirection`];
        /// * a `File`, which is a shorthand for `Redirection::File(file)`;
        /// * a `Vec<u8>` or `&str`, which will set up a `Redirection::Pipe`
        ///   for stdin, making sure that `capture` feeds that data into the
        ///   standard input of the subprocess.
        /// * `NullFile`, which will redirect the standard input to read from
        ///    /dev/null.
        ///
        /// [`Redirection`]: struct.Redirection.html
        pub fn stdin<T: IntoInputRedirection>(mut self, stdin: T)
                                              -> Pipeline {
            match stdin.into_input_redirection() {
                InputRedirection::AsRedirection(r) => self.stdin = r,
                InputRedirection::FeedData(data) => {
                    self.stdin = Redirection::Pipe;
                    self.stdin_data = Some(data);
                }
            };
            self
        }

        /// Specifies how to set up the standard output of the last
        /// command in the pipeline.
        ///
        /// Argument can be:
        ///
        /// * a [`Redirection`];
        /// * a `File`, which is a shorthand for `Redirection::File(file)`;
        /// * `NullFile`, which will redirect the standard input to read from
        ///    /dev/null.
        ///
        /// [`Redirection`]: struct.Redirection.html
        pub fn stdout<T: IntoOutputRedirection>(mut self, stdout: T)
                                                -> Pipeline {
            self.stdout = stdout.into_output_redirection();
            self
        }

        fn check_no_stdin_data(&self, meth: &str) {
            if self.stdin_data.is_some() {
                panic!("{} called with input data specified", meth);
            }
        }

        // Terminators:

        /// Starts all commands in the pipeline, and returns a
        /// `Vec<Popen>` whose members correspond to running commands.
        ///
        /// If some command fails to start, the remaining commands
        /// will not be started, and the appropriate error will be
        /// returned.  The commands that have already started will be
        /// waited to finish (but will probably exit immediately due
        /// to missing output), except for the ones for which
        /// `detached()` was called.  This is equivalent to what the
        /// shell does.
        pub fn popen(mut self) -> PopenResult<Vec<Popen>> {
            self.check_no_stdin_data("popen");
            assert!(self.cmds.len() >= 2);
            let cnt = self.cmds.len();

            let first_cmd = self.cmds.drain(..1).next().unwrap();
            self.cmds.insert(0, first_cmd.stdin(self.stdin));

            let last_cmd = self.cmds.drain(cnt - 1..).next().unwrap();
            self.cmds.push(last_cmd.stdout(self.stdout));

            let mut ret = Vec::<Popen>::new();

            for (idx, mut runner) in self.cmds.into_iter().enumerate() {
                if idx != 0 {
                    let prev_stdout = ret[idx - 1].stdout.take().unwrap();
                    runner = runner.stdin(prev_stdout);
                }
                if idx != cnt - 1 {
                    runner = runner.stdout(Redirection::Pipe);
                }
                ret.push(runner.popen()?);
            }
            Ok(ret)
        }

        /// Starts the pipeline, waits for it to finish, and returns
        /// the exit status of the last command.
        pub fn join(self) -> PopenResult<ExitStatus> {
            self.check_no_stdin_data("join");
            let mut v = self.popen()?;
            // Waiting on a pipeline waits for all commands, but
            // returns the status of the last one.  This is how the
            // shells do it.  If the caller needs more precise control
            // over which status is returned, they can call popen().
            v.last_mut().unwrap().wait()
        }

        /// Starts the pipeline and returns a `Read` trait object that
        /// reads from the standard output of the last command.
        ///
        /// This will automatically set up
        /// `stdout(Redirection::Pipe)`, so it is not necessary to do
        /// that beforehand.
        ///
        /// When the trait object is dropped, it will wait for the
        /// pipeline to finish.  If this is undesirable, use
        /// `detached()`.
        pub fn stream_stdout(self) -> PopenResult<Box<Read>> {
            self.check_no_stdin_data("stream_stdout");
            let v = self.stdout(Redirection::Pipe).popen()?;
            Ok(Box::new(ReadPipelineAdapter(v)))
        }

        /// Starts the pipeline and returns a `Write` trait object
        /// that writes to the standard input of the first command.
        ///
        /// This will automatically set up `stdin(Redirection::Pipe)`,
        /// so it is not necessary to do that beforehand.
        ///
        /// When the trait object is dropped, it will wait for the
        /// process to finish.  If this is undesirable, use
        /// `detached()`.
        pub fn stream_stdin(self) -> PopenResult<Box<Write>> {
            self.check_no_stdin_data("stream_stdin");
            let v = self.stdin(Redirection::Pipe).popen()?;
            Ok(Box::new(WritePipelineAdapter(v)))
        }

        /// Starts the pipeline, collects its output, and waits for
        /// all commands to finish.
        ///
        /// The return value provides the standard output of the last
        /// command error as bytes or optionally strings, as well as
        /// the exit status of the last command.
        ///
        /// Unlike `Popen::communicate`, this method actually waits
        /// for the processes to finish, rather than simply waiting
        /// for the output to close.  If this is undesirable, use
        /// `detached()`.
        pub fn capture(mut self) -> PopenResult<CaptureOutput> {
            assert!(self.cmds.len() >= 2);

            let stdin_data = self.stdin_data.take();
            let mut v = self.stdout(Redirection::Pipe).popen()?;

            let mut first = v.drain(..1).next().unwrap();
            let vlen = v.len();
            let mut last = v.drain(vlen - 1..).next().unwrap();

            let (maybe_out, _) = communicate::communicate(
                &mut first.stdin, &mut last.stdout, &mut None,
                stdin_data.as_ref().map(|v| &v[..]))?;
            let out = maybe_out.unwrap_or_else(Vec::new);

            let status = last.wait()?;

            Ok(CaptureOutput { stdout: out, exit_status: status })
        }
    }

    impl Clone for Pipeline {
        /// Returns a copy of the value.
        ///
        /// This method is guaranteed not to fail as long as none of
        /// the `Redirection` values contain a `Redirection::File`
        /// variant.  If a redirection to `File` is present, cloning
        /// that field will use `File::try_clone` method, which
        /// duplicates a file descriptor and can (but is not likely
        /// to) fail.  In that scenario, `Exec::clone` panics.
        fn clone(&self) -> Pipeline {
            Pipeline {
                cmds: self.cmds.clone(),
                stdin: self.stdin.try_clone().unwrap(),
                stdout: self.stdout.try_clone().unwrap(),
                stdin_data: self.stdin_data.clone()
            }
        }
    }

    impl BitOr<Exec> for Pipeline {
        type Output = Pipeline;

        /// Append a command to the pipeline and return a new pipeline.
        fn bitor(mut self, rhs: Exec) -> Pipeline {
            self.cmds.push(rhs);
            self
        }
    }

    impl BitOr for Pipeline {
        type Output = Pipeline;

        /// Append a pipeline to the pipeline and return a new pipeline.
        fn bitor(mut self, rhs: Pipeline) -> Pipeline {
            self.cmds.extend(rhs.cmds);
            self.stdout = rhs.stdout;
            self
        }
    }

    #[derive(Debug)]
    struct ReadPipelineAdapter(Vec<Popen>);

    impl Read for ReadPipelineAdapter {
        fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
            let last = self.0.last_mut().unwrap();
            last.stdout.as_mut().unwrap().read(buf)
        }
    }

    #[derive(Debug)]
    struct WritePipelineAdapter(Vec<Popen>);

    impl WritePipelineAdapter {
        fn stdin(&mut self) -> &mut File {
            let first = self.0.first_mut().unwrap();
            first.stdin.as_mut().unwrap()
        }
    }

    impl Write for WritePipelineAdapter {
        fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
            self.stdin().write(buf)
        }
        fn flush(&mut self) -> IoResult<()> {
            self.stdin().flush()
        }
    }

    impl Drop for WritePipelineAdapter {
        // the same rationale as Drop for WriteAdapter
        fn drop(&mut self) {
            let first = &mut self.0[0];
            first.stdin.take();
        }
    }

    /// Output of the last command in the pipeline.
    pub struct CaptureOutput {
        /// Output as bytes.
        pub stdout: Vec<u8>,
        /// Exit status of the pipeline.
        ///
        /// Following the shell convention, the exit status of the
        /// pipeline is defined as the exit status of the last command
        /// in the pipeline.  If you need the exit statuses of all
        /// processes, use `Pipeline::popen()` and collect the exit
        /// statuses e.g. with `map(Popen::wait).collect::<Vec<_>>()`.
        pub exit_status: ExitStatus
    }

    impl CaptureOutput {
        /// Returns pipeline output as string, converted from bytes
        /// using `String::from_utf8_lossy`.
        pub fn stdout_str(&self) -> String {
            String::from_utf8_lossy(&self.stdout).into_owned()
        }
    }
}
