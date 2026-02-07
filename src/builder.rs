#[cfg(unix)]
mod os {
    pub const SHELL: [&str; 2] = ["sh", "-c"];
}

#[cfg(windows)]
mod os {
    pub const SHELL: [&str; 2] = ["cmd.exe", "/c"];
}

#[cfg(unix)]
pub use exec::unix::ExecExt;
#[cfg(windows)]
pub use exec::windows::ExecExt;
pub use exec::{Capture, Exec, InputRedirection, OutputRedirection};
pub use pipeline::Pipeline;

/// Windows-specific process creation constants and extensions.
#[cfg(windows)]
pub mod windows {
    pub use super::exec::windows::*;
}

mod exec {
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
    use std::time::Duration;

    use crate::communicate::Communicator;
    use crate::popen::ExitStatus;
    use crate::popen::{Popen, PopenConfig, Redirection};

    use super::Pipeline;
    use super::os::*;

    /// A builder for [`Popen`] instances, providing control and convenience methods.
    ///
    /// `Exec` provides a builder API for [`Popen::create`], and includes convenience methods
    /// for capturing the output, and for connecting subprocesses into pipelines.
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
    /// Start a subprocess and obtain its output as a `Read` trait object, like C's `popen`:
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
    /// let out = Exec::cmd("ls")
    ///   .stdout(Redirection::Pipe)
    ///   .capture()?
    ///   .stdout_str();
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
    /// # fn dummy() -> std::io::Result<()> {
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
    #[must_use]
    pub struct Exec {
        command: OsString,
        args: Vec<OsString>,
        time_limit: Option<Duration>,
        config: PopenConfig,
        stdin_data: Option<Vec<u8>>,
    }

    impl Exec {
        /// Constructs a new `Exec`, configured to run `command`.
        ///
        /// The command will be run directly in the OS, without an intervening shell.  To run
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
                time_limit: None,
                config: PopenConfig::default(),
                stdin_data: None,
            }
        }

        /// Constructs a new `Exec`, configured to run `cmdstr` with the system shell.
        ///
        /// `subprocess` never spawns shells without an explicit request.  This command
        /// requests the shell to be used; on Unix-like systems, this is equivalent to
        /// `Exec::cmd("sh").arg("-c").arg(cmdstr)`.  On Windows, it runs
        /// `Exec::cmd("cmd.exe").arg("/c")`.
        ///
        /// `shell` is useful for porting code that uses the C `system` function, which also
        /// spawns a shell.
        ///
        /// When invoking this function, be careful not to interpolate arguments into the
        /// string run by the shell, such as `Exec::shell(format!("sort {}", filename))`.
        /// Such code is prone to errors and, if `filename` comes from an untrusted source, to
        /// shell injection attacks.  Instead, use `Exec::cmd("sort").arg(filename)`.
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
            self.config.detached = true;
            self
        }

        /// Limit the amount of time the next `read()` will spend reading from the
        /// subprocess.
        pub fn time_limit(mut self, time: Duration) -> Exec {
            self.time_limit = Some(time);
            self
        }

        fn ensure_env(&mut self) {
            if self.config.env.is_none() {
                self.config.env = Some(PopenConfig::current_env());
            }
        }

        /// Clears the environment of the subprocess.
        ///
        /// When this is invoked, the subprocess will not inherit the environment of this
        /// process.
        pub fn env_clear(mut self) -> Exec {
            self.config.env = Some(vec![]);
            self
        }

        /// Sets an environment variable in the child process.
        ///
        /// If the same variable is set more than once, the last value is used.
        ///
        /// Other environment variables are by default inherited from the current process.  If
        /// this is undesirable, call `env_clear` first.
        pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Exec {
            self.ensure_env();
            self.config
                .env
                .as_mut()
                .unwrap()
                .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
            self
        }

        /// Sets multiple environment variables in the child process.
        ///
        /// The keys and values of the variables are specified by the iterable.  If the same
        /// variable is set more than once, the last value is used.
        ///
        /// Other environment variables are by default inherited from the current process.  If
        /// this is undesirable, call `env_clear` first.
        pub fn env_extend(
            mut self,
            vars: impl IntoIterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>,
        ) -> Exec {
            self.ensure_env();
            {
                let envvec = self.config.env.as_mut().unwrap();
                envvec.extend(
                    vars.into_iter()
                        .map(|(k, v)| (k.as_ref().to_owned(), v.as_ref().to_owned())),
                );
            }
            self
        }

        /// Removes an environment variable from the child process.
        ///
        /// Other environment variables are inherited by default.
        pub fn env_remove(mut self, key: impl AsRef<OsStr>) -> Exec {
            self.ensure_env();
            self.config
                .env
                .as_mut()
                .unwrap()
                .retain(|(k, _v)| k != key.as_ref());
            self
        }

        /// Specifies the current working directory of the child process.
        ///
        /// If unspecified, the current working directory is inherited from the parent.
        pub fn cwd(mut self, dir: impl AsRef<Path>) -> Exec {
            self.config.cwd = Some(dir.as_ref().as_os_str().to_owned());
            self
        }

        /// Specifies how to set up the standard input of the child process.
        ///
        /// Argument can be:
        ///
        /// * a [`Redirection`], including [`Redirection::Null`] to redirect
        ///   to the null device;
        /// * a `File`, which is a shorthand for `Redirection::File(file)`;
        /// * a `Vec<u8>` or `&str`, which will set up a `Redirection::Pipe`
        ///   for stdin, making sure that `capture` feeds that data into the
        ///   standard input of the subprocess.
        ///
        /// [`Redirection`]: enum.Redirection.html
        /// [`Redirection::Null`]: enum.Redirection.html#variant.Null
        pub fn stdin(mut self, stdin: impl InputRedirection) -> Exec {
            match (&self.config.stdin, stdin.into_input_redirection()) {
                (&Redirection::None, InputRedirectionKind::AsRedirection(new)) => {
                    self.config.stdin = new
                }
                (&Redirection::Pipe, InputRedirectionKind::AsRedirection(Redirection::Pipe)) => (),
                (&Redirection::None, InputRedirectionKind::FeedData(data)) => {
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
        /// * a [`Redirection`], including [`Redirection::Null`] to redirect
        ///   to the null device;
        /// * a `File`, which is a shorthand for `Redirection::File(file)`.
        ///
        /// [`Redirection`]: enum.Redirection.html
        /// [`Redirection::Null`]: enum.Redirection.html#variant.Null
        pub fn stdout(mut self, stdout: impl OutputRedirection) -> Exec {
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
        /// * a [`Redirection`], including [`Redirection::Null`] to redirect
        ///   to the null device;
        /// * a `File`, which is a shorthand for `Redirection::File(file)`.
        ///
        /// [`Redirection`]: enum.Redirection.html
        /// [`Redirection::Null`]: enum.Redirection.html#variant.Null
        pub fn stderr(mut self, stderr: impl OutputRedirection) -> Exec {
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
        pub fn popen(mut self) -> io::Result<Popen> {
            self.check_no_stdin_data("popen");
            self.args.insert(0, self.command);
            let p = Popen::create(&self.args, self.config)?;
            Ok(p)
        }

        /// Starts the process, waits for it to finish, and returns the exit status.
        ///
        /// This method will wait for as long as necessary for the process to finish.  If a
        /// timeout is needed, use `<...>.detached().popen()?.wait_timeout(...)` instead.
        pub fn join(self) -> io::Result<ExitStatus> {
            self.check_no_stdin_data("join");
            self.popen()?.wait()
        }

        /// Starts the process and returns a value implementing the `Read` trait that reads from
        /// the standard output of the child process.
        ///
        /// This will automatically set up `stdout(Redirection::Pipe)`, so it is not necessary
        /// to do that beforehand.
        ///
        /// When the trait object is dropped, it will wait for the process to finish.  If this
        /// is undesirable, use `detached()`.
        pub fn stream_stdout(self) -> io::Result<impl Read> {
            self.check_no_stdin_data("stream_stdout");
            let p = self.stdout(Redirection::Pipe).popen()?;
            Ok(ReadOutAdapter(p))
        }

        /// Starts the process and returns a value implementing the `Read` trait that reads from
        /// the standard error of the child process.
        ///
        /// This will automatically set up `stderr(Redirection::Pipe)`, so it is not necessary
        /// to do that beforehand.
        ///
        /// When the trait object is dropped, it will wait for the process to finish.  If this
        /// is undesirable, use `detached()`.
        pub fn stream_stderr(self) -> io::Result<impl Read> {
            self.check_no_stdin_data("stream_stderr");
            let p = self.stderr(Redirection::Pipe).popen()?;
            Ok(ReadErrAdapter(p))
        }

        /// Starts the process and returns a value implementing the `Write` trait that writes to
        /// the standard input of the child process.
        ///
        /// This will automatically set up `stdin(Redirection::Pipe)`, so it is not necessary
        /// to do that beforehand.
        ///
        /// When the trait object is dropped, it will wait for the process to finish.  If this
        /// is undesirable, use `detached()`.
        pub fn stream_stdin(self) -> io::Result<impl Write> {
            self.check_no_stdin_data("stream_stdin");
            let p = self.stdin(Redirection::Pipe).popen()?;
            Ok(WriteAdapter(p))
        }

        fn setup_communicate(mut self) -> io::Result<(Communicator<Vec<u8>>, Popen)> {
            let stdin_data = self.stdin_data.take();
            if let (&Redirection::None, &Redirection::None) =
                (&self.config.stdout, &self.config.stderr)
            {
                self = self.stdout(Redirection::Pipe);
            }
            let mut p = self.popen()?;

            let comm = Communicator::new(
                p.stdin.take(),
                p.stdout.take(),
                p.stderr.take(),
                stdin_data.unwrap_or_default(),
            );
            Ok((comm, p))
        }

        /// Starts the process and returns a `Communicator` handle.
        ///
        /// Compared to `capture()`, this offers more choice in how communication is
        /// performed, such as read size limit and timeout.
        ///
        /// Unlike `capture()`, this method doesn't wait for the process to finish,
        /// effectively detaching it.
        pub fn communicate(self) -> io::Result<Communicator<Vec<u8>>> {
            let comm = self.detached().setup_communicate()?.0;
            Ok(comm)
        }

        /// Starts the process, collects its output, and waits for it to finish.
        ///
        /// The return value provides the standard output and standard error as bytes or
        /// optionally strings, as well as the exit status.
        ///
        /// This method waits for the process to finish, rather than simply waiting for
        /// its standard streams to close.  If this is undesirable, use `detached()`.
        pub fn capture(self) -> io::Result<Capture> {
            let timeout = self.time_limit;
            let (mut comm, mut p) = self.setup_communicate()?;
            if let Some(t) = timeout {
                comm = comm.limit_time(t);
            }

            let (stdout, stderr) = comm.read()?;
            Ok(Capture {
                stdout,
                stderr,
                exit_status: match timeout {
                    Some(t) => p
                        .wait_timeout(t)?
                        .ok_or(io::Error::from(ErrorKind::TimedOut))?,
                    None => p.wait()?,
                },
            })
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
            if let Some(ref cmd_env) = self.config.env {
                let current: Vec<_> = env::vars_os().collect();
                let current_map: HashMap<_, _> = current.iter().map(|(x, y)| (x, y)).collect();
                for (k, v) in cmd_env {
                    if current_map.get(&k) == Some(&v) {
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

        pub(super) fn stdin_is_set(&self) -> bool {
            !matches!(self.config.stdin, Redirection::None)
        }

        pub(super) fn stdout_is_set(&self) -> bool {
            !matches!(self.config.stdout, Redirection::None)
        }
    }

    impl Clone for Exec {
        /// Returns a copy of the value.
        ///
        /// This method is guaranteed not to fail as long as none of the `Redirection` values
        /// contain a `Redirection::File` variant.  If a redirection to `File` is present,
        /// cloning that field will use `File::try_clone` method, which duplicates a file
        /// descriptor and can (but is not likely to) fail.  In that scenario, `Exec::clone`
        /// panics.
        fn clone(&self) -> Exec {
            Exec {
                command: self.command.clone(),
                args: self.args.clone(),
                time_limit: self.time_limit,
                config: self.config.try_clone().unwrap(),
                stdin_data: self.stdin_data.clone(),
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

    impl fmt::Debug for Exec {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Exec {{ {} }}", self.to_cmdline_lossy())
        }
    }

    #[derive(Debug)]
    struct ReadOutAdapter(Popen);

    impl Read for ReadOutAdapter {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.stdout.as_mut().unwrap().read(buf)
        }
    }

    #[derive(Debug)]
    struct ReadErrAdapter(Popen);

    impl Read for ReadErrAdapter {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.stderr.as_mut().unwrap().read(buf)
        }
    }

    #[derive(Debug)]
    struct WriteAdapter(Popen);

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
        use super::Exec;

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
            /// leader of a new process group.  Use with [`PopenExt::send_signal_group`]
            /// to signal the entire group.
            fn setpgid(self) -> Self;
        }

        impl ExecExt for Exec {
            fn setuid(mut self, uid: u32) -> Exec {
                self.config.setuid = Some(uid);
                self
            }

            fn setgid(mut self, gid: u32) -> Exec {
                self.config.setgid = Some(gid);
                self
            }

            fn setpgid(mut self) -> Exec {
                self.config.setpgid = true;
                self
            }
        }
    }

    #[cfg(windows)]
    pub mod windows {
        use super::Exec;

        /// Process creation flag: The process does not have a console window.
        ///
        /// Use this flag when launching GUI applications or background processes to prevent
        /// a console window from briefly appearing.
        pub const CREATE_NO_WINDOW: u32 = 0x08000000;

        /// Process creation flag: The new process has a new console.
        ///
        /// This flag cannot be used with `DETACHED_PROCESS`.
        pub const CREATE_NEW_CONSOLE: u32 = 0x00000010;

        /// Process creation flag: The new process is the root of a new process group.
        ///
        /// The process group includes all descendant processes. Useful for sending signals
        /// to a group of related processes.
        pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;

        /// Process creation flag: The process does not inherit its parent's console.
        ///
        /// The new process can call `AllocConsole` later to create a console.
        /// This flag cannot be used with `CREATE_NEW_CONSOLE`.
        pub const DETACHED_PROCESS: u32 = 0x00000008;

        /// Extension trait for Windows-specific process creation options.
        pub trait ExecExt {
            /// Set process creation flags for Windows.
            ///
            /// This value is passed to the `dwCreationFlags` parameter of `CreateProcessW`.
            /// Use this to control process creation behavior such as creating the process
            /// without a console window.
            ///
            /// # Example
            ///
            /// ```ignore
            /// use subprocess::{Exec, ExecExt, windows::CREATE_NO_WINDOW};
            ///
            /// let popen = Exec::cmd("my_app")
            ///     .creation_flags(CREATE_NO_WINDOW)
            ///     .popen()?;
            /// ```
            fn creation_flags(self, flags: u32) -> Self;
        }

        impl ExecExt for Exec {
            fn creation_flags(mut self, flags: u32) -> Exec {
                self.config.creation_flags = flags;
                self
            }
        }
    }
}

mod pipeline {
    use std::fmt;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::ops::BitOr;
    use std::sync::Arc;

    use crate::communicate::Communicator;
    use crate::popen::ExitStatus;
    use crate::popen::{Popen, Redirection};

    use super::exec::{Capture, Exec, InputRedirection, InputRedirectionKind, OutputRedirection};

    /// A builder for multiple [`Popen`] instances connected via pipes.
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
    ///     Exec::cmd("find . -type f") | Exec::cmd("sort") | Exec::cmd("sha1sum")
    /// }.capture()?.stdout_str();
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// [`Popen`]: struct.Popen.html
    /// [`Exec`]: struct.Exec.html
    /// [`Pipeline`]: struct.Pipeline.html
    #[must_use]
    pub struct Pipeline {
        cmds: Vec<Exec>,
        stdin: Redirection,
        stdout: Redirection,
        stderr_file: Option<File>,
        stdin_data: Option<Vec<u8>>,
    }

    impl Pipeline {
        /// Creates a new pipeline by combining two commands.
        ///
        /// Equivalent to `cmd1 | cmd2`.
        ///
        /// # Panics
        ///
        /// Panics if `cmd1` has stdin redirected or `cmd2` has stdout redirected.
        /// Use `Pipeline::stdin()` and `Pipeline::stdout()` to redirect the pipeline's streams.
        pub fn new(cmd1: Exec, cmd2: Exec) -> Pipeline {
            if cmd1.stdin_is_set() {
                panic!(
                    "stdin of the first command is already redirected; \
                     use Pipeline::stdin() to redirect pipeline input"
                );
            }
            if cmd2.stdout_is_set() {
                panic!(
                    "stdout of the last command is already redirected; \
                     use Pipeline::stdout() to redirect pipeline output"
                );
            }
            Pipeline {
                cmds: vec![cmd1, cmd2],
                stdin: Redirection::None,
                stdout: Redirection::None,
                stderr_file: None,
                stdin_data: None,
            }
        }

        /// Creates a new pipeline from a list of commands.  Useful if a pipeline should be
        /// created dynamically.
        ///
        /// # Panics
        ///
        /// Panics if:
        /// - The iterator contains fewer than two commands.
        /// - The first command has stdin redirected.
        /// - The last command has stdout redirected.
        ///
        /// Use `Pipeline::stdin()` and `Pipeline::stdout()` to redirect the pipeline's streams.
        ///
        /// # Example
        ///
        /// ```no_run
        /// use subprocess::Exec;
        ///
        /// let commands = vec![
        ///   Exec::shell("echo tset"),
        ///   Exec::shell("tr '[:lower:]' '[:upper:]'"),
        ///   Exec::shell("rev")
        /// ];
        ///
        /// let pipeline = subprocess::Pipeline::from_exec_iter(commands);
        /// let output = pipeline.capture().unwrap().stdout_str();
        /// assert_eq!(output, "TEST\n");
        /// ```
        pub fn from_exec_iter<I>(iterable: I) -> Pipeline
        where
            I: IntoIterator<Item = Exec>,
        {
            let cmds: Vec<_> = iterable.into_iter().collect();

            if cmds.len() < 2 {
                panic!("pipeline requires at least two commands")
            }
            if cmds.first().unwrap().stdin_is_set() {
                panic!(
                    "stdin of the first command is already redirected; \
                     use Pipeline::stdin() to redirect pipeline input"
                );
            }
            if cmds.last().unwrap().stdout_is_set() {
                panic!(
                    "stdout of the last command is already redirected; \
                     use Pipeline::stdout() to redirect pipeline output"
                );
            }

            Pipeline {
                cmds,
                stdin: Redirection::None,
                stdout: Redirection::None,
                stderr_file: None,
                stdin_data: None,
            }
        }

        /// Specifies how to set up the standard input of the first command in the pipeline.
        ///
        /// Argument can be:
        ///
        /// * a [`Redirection`], including [`Redirection::Null`] to redirect
        ///   to the null device;
        /// * a `File`, which is a shorthand for `Redirection::File(file)`;
        /// * a `Vec<u8>` or `&str`, which will set up a `Redirection::Pipe`
        ///   for stdin, making sure that `capture` feeds that data into the
        ///   standard input of the subprocess.
        ///
        /// [`Redirection`]: enum.Redirection.html
        /// [`Redirection::Null`]: enum.Redirection.html#variant.Null
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
        /// * a [`Redirection`], including [`Redirection::Null`] to redirect
        ///   to the null device;
        /// * a `File`, which is a shorthand for `Redirection::File(file)`.
        ///
        /// [`Redirection`]: enum.Redirection.html
        /// [`Redirection::Null`]: enum.Redirection.html#variant.Null
        pub fn stdout(mut self, stdout: impl OutputRedirection) -> Pipeline {
            self.stdout = stdout.into_output_redirection();
            self
        }

        /// Specifies a file to which to redirect the standard error of all the commands in the
        /// pipeline.
        ///
        /// It is useful for capturing the standard error of the pipeline as a whole.  Unlike
        /// `stdout()`, which only affects the last command in the pipeline, this affects all
        /// commands.  The difference is because standard output is piped from one command to
        /// the next, so only the output of the last command is "free".  In contrast, the
        /// standard errors are not connected in any way.  This is also the reason only a
        /// `File` is supported - it allows for efficient sharing of the same file by all
        /// commands.
        ///
        /// Note that this differs from the shell's `cmd1 | cmd2 2>file`, which only
        /// redirects stderr of the last command.  This method is equivalent to
        /// `(cmd1 | cmd2) 2>file`, but without the overhead of a subshell.
        pub fn stderr_to(mut self, to: File) -> Pipeline {
            self.stderr_file = Some(to);
            self
        }

        fn check_no_stdin_data(&self, meth: &str) {
            if self.stdin_data.is_some() {
                panic!("{} called with input data specified", meth);
            }
        }

        // Terminators:

        /// Starts all commands in the pipeline, and returns a `Vec<Popen>` whose members
        /// correspond to running commands.
        ///
        /// If some command fails to start, the remaining commands will not be started, and
        /// the appropriate error will be returned.  The commands that have already started
        /// will be waited to finish (but will probably exit immediately due to missing
        /// output), except for the ones for which `detached()` was called.  This is
        /// equivalent to what the shell does.
        pub fn popen(mut self) -> io::Result<Vec<Popen>> {
            self.check_no_stdin_data("popen");
            assert!(self.cmds.len() >= 2);

            if let Some(stderr_to) = self.stderr_file {
                let stderr_to = Arc::new(stderr_to);
                self.cmds = self
                    .cmds
                    .into_iter()
                    .map(|cmd| cmd.stderr(Redirection::SharedFile(Arc::clone(&stderr_to))))
                    .collect();
            }

            let first_cmd = self.cmds.remove(0);
            self.cmds.insert(0, first_cmd.stdin(self.stdin));

            let last_cmd = self.cmds.remove(self.cmds.len() - 1);
            self.cmds.push(last_cmd.stdout(self.stdout));

            let mut ret = Vec::<Popen>::new();
            let cnt = self.cmds.len();

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

        /// Starts the pipeline, waits for it to finish, and returns the exit status of the
        /// last command.
        pub fn join(self) -> io::Result<ExitStatus> {
            self.check_no_stdin_data("join");
            let mut v = self.popen()?;
            // Waiting on a pipeline waits for all commands, but returns the status of the
            // last one.  This is how the shells do it.  If the caller needs more precise
            // control over which status is returned, they can call popen().
            v.last_mut().unwrap().wait()
        }

        /// Starts the pipeline and returns a value implementing the `Read` trait that reads
        /// from the standard output of the last command.
        ///
        /// This will automatically set up `stdout(Redirection::Pipe)`, so it is not necessary
        /// to do that beforehand.
        ///
        /// When the trait object is dropped, it will wait for the pipeline to finish.  If
        /// this is undesirable, use `detached()`.
        pub fn stream_stdout(self) -> io::Result<impl Read> {
            self.check_no_stdin_data("stream_stdout");
            let v = self.stdout(Redirection::Pipe).popen()?;
            Ok(ReadPipelineAdapter(v))
        }

        /// Starts the pipeline and returns a value implementing the `Write` trait that writes
        /// to the standard input of the first command.
        ///
        /// This will automatically set up `stdin(Redirection::Pipe)`, so it is not necessary
        /// to do that beforehand.
        ///
        /// When the trait object is dropped, it will wait for the process to finish.  If this
        /// is undesirable, use `detached()`.
        pub fn stream_stdin(self) -> io::Result<impl Write> {
            self.check_no_stdin_data("stream_stdin");
            let v = self.stdin(Redirection::Pipe).popen()?;
            Ok(WritePipelineAdapter(v))
        }

        fn setup_communicate(mut self) -> io::Result<(Communicator<Vec<u8>>, Vec<Popen>)> {
            assert!(self.cmds.len() >= 2);

            // Parent reads stderr - make_pipe() creates pipes suitable for this
            let (err_read, err_write) = crate::popen::make_pipe()?;
            self = self.stderr_to(err_write);

            let stdin_data = self.stdin_data.take();
            let mut v = self.stdout(Redirection::Pipe).popen()?;
            let vlen = v.len();

            let comm = Communicator::new(
                v[0].stdin.take(),
                v[vlen - 1].stdout.take(),
                Some(err_read),
                stdin_data.unwrap_or_default(),
            );
            Ok((comm, v))
        }

        /// Starts the pipeline and returns a `Communicator` handle.
        ///
        /// Compared to `capture()`, this offers more choice in how communication is
        /// performed, such as read size limit and timeout.
        ///
        /// Unlike `capture()`, this method doesn't wait for the pipeline to finish,
        /// effectively detaching it.
        pub fn communicate(mut self) -> io::Result<Communicator<Vec<u8>>> {
            self.cmds = self.cmds.into_iter().map(|cmd| cmd.detached()).collect();
            let comm = self.setup_communicate()?.0;
            Ok(comm)
        }

        /// Starts the pipeline, collects its output, and waits for all commands to finish.
        ///
        /// The return value provides the standard output of the last command, the combined
        /// standard error of all commands, and the exit status of the last command.  The
        /// captured outputs can be accessed as bytes or strings.
        ///
        /// This method actually waits for the processes to finish, rather than simply
        /// waiting for the output to close.  If this is undesirable, use `detached()`.
        pub fn capture(self) -> io::Result<Capture> {
            let (mut comm, mut v) = self.setup_communicate()?;
            let (stdout, stderr) = comm.read()?;

            let vlen = v.len();
            let status = v[vlen - 1].wait()?;

            Ok(Capture {
                stdout,
                stderr,
                exit_status: status,
            })
        }
    }

    impl Clone for Pipeline {
        /// Returns a copy of the value.
        ///
        /// This method is guaranteed not to fail as long as none of the `Redirection` values
        /// contain a `Redirection::File` variant.  If a redirection to `File` is present,
        /// cloning that field will use `File::try_clone` method, which duplicates a file
        /// descriptor and can (but is not likely to) fail.  In that scenario, `Pipeline::clone`
        /// panics.
        fn clone(&self) -> Pipeline {
            Pipeline {
                cmds: self.cmds.clone(),
                stdin: self.stdin.try_clone().unwrap(),
                stdout: self.stdout.try_clone().unwrap(),
                stderr_file: self.stderr_file.as_ref().map(|f| f.try_clone().unwrap()),
                stdin_data: self.stdin_data.clone(),
            }
        }
    }

    impl BitOr<Exec> for Pipeline {
        type Output = Pipeline;

        /// Append a command to the pipeline and return a new pipeline.
        ///
        /// # Panics
        ///
        /// Panics if the new command has stdout redirected.
        fn bitor(mut self, rhs: Exec) -> Pipeline {
            if rhs.stdout_is_set() {
                panic!(
                    "stdout of the last command is already redirected; \
                     use Pipeline::stdout() to redirect pipeline output"
                );
            }
            self.cmds.push(rhs);
            self
        }
    }

    impl BitOr for Pipeline {
        type Output = Pipeline;

        /// Append a pipeline to the pipeline and return a new pipeline.
        ///
        /// # Panics
        ///
        /// Panics if the last command of `rhs` has stdout redirected.
        fn bitor(mut self, rhs: Pipeline) -> Pipeline {
            if rhs.cmds.last().unwrap().stdout_is_set() {
                panic!(
                    "stdout of the last command is already redirected; \
                     use Pipeline::stdout() to redirect pipeline output"
                );
            }
            self.cmds.extend(rhs.cmds);
            self.stdout = rhs.stdout;
            self
        }
    }

    impl fmt::Debug for Pipeline {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let mut args = vec![];
            for cmd in &self.cmds {
                args.push(cmd.to_cmdline_lossy());
            }
            write!(f, "Pipeline {{ {} }}", args.join(" | "))
        }
    }

    #[derive(Debug)]
    struct ReadPipelineAdapter(Vec<Popen>);

    impl Read for ReadPipelineAdapter {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.stdin().write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.stdin().flush()
        }
    }
}
