use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::io::ErrorKind;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::communicate;
use crate::os_common::{ExitStatus, StandardStream};

use ChildState::*;

pub use communicate::Communicator;
#[cfg(unix)] // only consumed by lib.rs's `unix` module, which only exists on Unix
pub use os::ext as os_ext;
pub use os::make_pipe;

/// Interface to a running subprocess.
///
/// `Popen` is the parent's interface to a created subprocess.  The child process is started
/// in the constructor, so owning a `Popen` value indicates that the specified program has
/// been successfully launched.  To prevent accumulation of zombie processes, the child is
/// waited upon when a `Popen` goes out of scope, which can be prevented using the [`detach`]
/// method.
///
/// Depending on how the subprocess was configured, its input, output, and error streams can
/// be connected to the parent and available as [`stdin`], [`stdout`], and [`stderr`] public
/// fields.  If you need to read the output and errors into memory (or provide input as a
/// memory slice), use the [`communicate`] family of methods.
///
/// `Popen` instances can be obtained with the [`create`] method, or using the [`popen`]
/// method of the [`Exec`] type.  Subprocesses can be connected into pipes, most easily
/// achieved using using [`Exec`].
///
/// [`Exec`]: struct.Exec.html
/// [`popen`]: struct.Exec.html#method.popen
/// [`stdin`]: struct.Popen.html#structfield.stdin
/// [`stdout`]: struct.Popen.html#structfield.stdout
/// [`stderr`]: struct.Popen.html#structfield.stderr
/// [`create`]: struct.Popen.html#method.create
/// [`communicate`]: struct.Popen.html#method.communicate
/// [`detach`]: struct.Popen.html#method.detach

#[derive(Debug)]
pub struct Popen {
    /// If `stdin` was specified as `Redirection::Pipe`, this will contain a writable `File`
    /// connected to the standard input of the child process.
    pub stdin: Option<File>,

    /// If `stdout` was specified as `Redirection::Pipe`, this will contain a readable `File`
    /// connected to the standard output of the child process.
    pub stdout: Option<File>,

    /// If `stderr` was specified as `Redirection::Pipe`, this will contain a readable `File`
    /// connected to the standard error of the child process.
    pub stderr: Option<File>,

    child_state: ChildState,
    detached: bool,
}

#[derive(Debug)]
enum ChildState {
    Preparing, // only during construction
    Running {
        pid: u32,
        #[allow(dead_code)]
        ext: os::ExtChildState,
    },
    Finished(ExitStatus),
}

/// Options for [`Popen::create`].
///
/// When constructing `PopenConfig`, always use the [`Default`] trait, such as:
///
/// ```
/// # use subprocess::*;
/// # let argv = &["true"];
/// Popen::create(argv, PopenConfig {
///      stdout: Redirection::Pipe,
///      detached: true,
///      // ... other fields you want to override ...
///      ..Default::default()
/// })
/// # .unwrap();
/// ```
///
/// This ensures that fields added later do not break existing code.
///
/// An alternative to using `PopenConfig` directly is creating processes using [`Exec`], a
/// builder for `Popen`.
///
/// [`Popen::create`]: struct.Popen.html#method.create
/// [`Exec`]: struct.Exec.html
/// [`Default`]: https://doc.rust-lang.org/core/default/trait.Default.html

#[derive(Debug)]
pub struct PopenConfig {
    /// How to configure the executed program's standard input.
    pub stdin: Redirection,
    /// How to configure the executed program's standard output.
    pub stdout: Redirection,
    /// How to configure the executed program's standard error.
    pub stderr: Redirection,
    /// Whether the `Popen` instance is initially detached.
    pub detached: bool,
    /// Process creation flags for Windows.
    ///
    /// This value is passed to the `dwCreationFlags` parameter of
    /// [`CreateProcessW`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw).
    /// Use this to control process creation behavior such as creating the process without a
    /// console window.
    ///
    /// For example, to prevent a console window from appearing when spawning a GUI application
    /// or background process:
    ///
    /// ```ignore
    /// use subprocess::{Popen, PopenConfig, windows::CREATE_NO_WINDOW};
    ///
    /// let popen = Popen::create(&["my_app"], PopenConfig {
    ///     creation_flags: CREATE_NO_WINDOW,
    ///     ..Default::default()
    /// })?;
    /// ```
    ///
    /// Available flags are in the [`windows`](crate::windows) module: `CREATE_NO_WINDOW`,
    /// `CREATE_NEW_CONSOLE`, `CREATE_NEW_PROCESS_GROUP`, `DETACHED_PROCESS`.
    /// See Windows documentation for the full list.
    #[cfg(windows)]
    pub creation_flags: u32,

    /// Executable to run.
    ///
    /// If provided, this executable will be used to run the program instead of `argv[0]`.
    /// However, `argv[0]` will still be passed to the subprocess, which will see that as
    /// `argv[0]`.  On some Unix systems, `ps` will show the string passed as `argv[0]`, even
    /// though `executable` is actually running.
    pub executable: Option<OsString>,

    /// Environment variables to pass to the subprocess.
    ///
    /// If this is None, environment variables are inherited from the calling process.
    /// Otherwise, the specified variables are used instead.
    ///
    /// Duplicates are eliminated, with the value taken from the variable appearing later in
    /// the vector.
    pub env: Option<Vec<(OsString, OsString)>>,

    /// Initial current working directory of the subprocess.
    ///
    /// None means inherit the working directory from the parent.
    pub cwd: Option<OsString>,

    /// Set user ID for the subprocess.
    ///
    /// If specified, calls `setuid()` before execing the child process.
    #[cfg(unix)]
    pub setuid: Option<u32>,

    /// Set group ID for the subprocess.
    ///
    /// If specified, calls `setgid()` before execing the child process.
    ///
    /// Not to be confused with similarly named `setpgid`.
    #[cfg(unix)]
    pub setgid: Option<u32>,

    /// Make the subprocess belong to a new process group.
    ///
    /// If specified, calls `setpgid(0, 0)` before execing the child process.
    ///
    /// Not to be confused with similarly named `setgid`.
    #[cfg(unix)]
    pub setpgid: bool,

    // Add this field to force construction using ..Default::default() for backward
    // compatibility.  Unfortunately we can't mark this non-public because then
    // ..Default::default() wouldn't work either.
    #[doc(hidden)]
    pub _use_default_to_construct: (),
}

impl PopenConfig {
    /// Clone the underlying [`PopenConfig`], or return an error.
    ///
    /// This is guaranteed not to fail as long as no [`Redirection::File`] variant is used for
    /// one of the standard streams.  Otherwise, it fails if `File::try_clone` fails on one of
    /// the `Redirection`s.
    ///
    /// [`PopenConfig`]: struct.PopenConfig.html
    /// [`Redirection::File`]: enum.Redirection.html#variant.File
    pub fn try_clone(&self) -> io::Result<PopenConfig> {
        Ok(PopenConfig {
            stdin: self.stdin.try_clone()?,
            stdout: self.stdout.try_clone()?,
            stderr: self.stderr.try_clone()?,
            detached: self.detached,
            #[cfg(windows)]
            creation_flags: self.creation_flags,
            executable: self.executable.clone(),
            env: self.env.clone(),
            cwd: self.cwd.clone(),
            #[cfg(unix)]
            setuid: self.setuid,
            #[cfg(unix)]
            setgid: self.setgid,
            #[cfg(unix)]
            setpgid: self.setpgid,
            _use_default_to_construct: (),
        })
    }

    /// Returns the environment of the current process.
    ///
    /// The returned value is in the format accepted by the `env`
    /// member of `PopenConfig`.
    pub fn current_env() -> Vec<(OsString, OsString)> {
        env::vars_os().collect()
    }
}

impl Default for PopenConfig {
    fn default() -> PopenConfig {
        PopenConfig {
            stdin: Redirection::None,
            stdout: Redirection::None,
            stderr: Redirection::None,
            detached: false,
            #[cfg(windows)]
            creation_flags: 0,
            executable: None,
            env: None,
            cwd: None,
            #[cfg(unix)]
            setuid: None,
            #[cfg(unix)]
            setgid: None,
            #[cfg(unix)]
            setpgid: false,
            _use_default_to_construct: (),
        }
    }
}

/// Instruction what to do with a stream in the child process.
///
/// `Redirection` values are used for the `stdin`, `stdout`, and `stderr` field of the
/// `PopenConfig` struct.  They tell `Popen::create` how to set up the standard streams in the
/// child process and the corresponding fields of the `Popen` struct in the parent.

#[derive(Debug)]
pub enum Redirection {
    /// Do nothing with the stream.
    ///
    /// The stream is typically inherited from the parent.  The field in `Popen` corresponding
    /// to the stream will be `None`.
    None,

    /// Redirect the stream to a pipe.
    ///
    /// This variant requests that a stream be redirected to a unidirectional pipe.  One end
    /// of the pipe is passed to the child process and configured as one of its standard
    /// streams, and the other end is available to the parent for communicating with the
    /// child.
    ///
    /// The field with `Popen` corresponding to the stream will be `Some(file)`, `File` being
    /// the parent's end of the pipe.
    Pipe,

    /// Merge the stream to the other output stream.
    ///
    /// This variant is only valid when configuring redirection of standard output and
    /// standard error.  Using `Redirection::Merge` for `PopenConfig::stderr` requests the
    /// child's stderr to refer to the same underlying file as the child's stdout (which may
    /// or may not itself be redirected), equivalent to the `2>&1` operator of the Bourne
    /// shell.  Analogously, using `Redirection::Merge` for `PopenConfig::stdout` is
    /// equivalent to `1>&2` in the shell.
    ///
    /// Specifying `Redirection::Merge` for `PopenConfig::stdin` or specifying it for both
    /// `stdout` and `stderr` is invalid and will cause `Popen::create` to return
    /// `Err` with `io::ErrorKind::InvalidInput`.
    ///
    /// The field in `Popen` corresponding to the stream will be `None`.
    Merge,

    /// Redirect the stream to the specified open `File`.
    ///
    /// This does not create a pipe, it simply spawns the child so that the specified stream
    /// sees that file.  The child can read from or write to the provided file on its own,
    /// without any intervention by the parent.
    ///
    /// The field in `Popen` corresponding to the stream will be `None`.
    File(File),

    /// Like `File`, but the file may be shared among multiple redirections without
    /// duplicating the file descriptor.
    SharedFile(Arc<File>),
}

impl Redirection {
    /// Clone the underlying `Redirection`, or return an error.
    ///
    /// Can fail in `File` variant.
    pub fn try_clone(&self) -> io::Result<Redirection> {
        Ok(match *self {
            Redirection::None => Redirection::None,
            Redirection::Pipe => Redirection::Pipe,
            Redirection::Merge => Redirection::Merge,
            Redirection::File(ref f) => Redirection::File(f.try_clone()?),
            Redirection::SharedFile(ref f) => Redirection::SharedFile(Arc::clone(f)),
        })
    }
}

impl Popen {
    /// Execute an external program in a new process.
    ///
    /// `argv` is a slice containing the program followed by its arguments, such as
    /// `&["ps", "x"]`. `config` specifies details how to create and interface to the process.
    ///
    /// For example, this launches the `cargo update` command:
    ///
    /// ```no_run
    /// # use subprocess::*;
    /// # fn dummy() -> Result<()> {
    /// Popen::create(&["cargo", "update"], PopenConfig::default())?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// If the external program cannot be executed for any reason, an error is returned.  The
    /// most typical reason for execution to fail is that the program is missing on the
    /// `PATH`, but other errors are also possible.  Note that this is distinct from the
    /// program running and then exiting with a failure code - this can be detected by calling
    /// the `wait` method to obtain its exit status.
    pub fn create(argv: &[impl AsRef<OsStr>], config: PopenConfig) -> Result<Popen> {
        if argv.is_empty() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "argv must not be empty",
            ));
        }
        let argv: Vec<OsString> = argv.iter().map(|p| p.as_ref().to_owned()).collect();
        let mut inst = Popen {
            stdin: None,
            stdout: None,
            stderr: None,
            child_state: ChildState::Preparing,
            detached: config.detached,
        };
        inst.os_start(argv, config)?;
        Ok(inst)
    }

    // Create the pipes requested by stdin, stdout, and stderr from the PopenConfig used
    // to construct us, and return the Files to be given to the child process.
    //
    // For Redirection::Pipe, this stores the parent end of the pipe to the appropriate
    // self.std* field, and returns the child end of the pipe.
    //
    // For Redirection::File, this transfers the ownership of the File to the
    // corresponding child.
    fn setup_streams(
        &mut self,
        stdin: Redirection,
        stdout: Redirection,
        stderr: Redirection,
    ) -> Result<(Option<Arc<File>>, Option<Arc<File>>, Option<Arc<File>>)> {
        fn prepare_pipe(
            parent_writes: bool,
            parent_ref: &mut Option<File>,
            child_ref: &mut Option<Arc<File>>,
        ) -> Result<()> {
            // Store the parent's end of the pipe into the given reference, and store the
            // child end. On Windows, this creates pipes where both ends support
            // overlapped I/O (see make_pipe() for details).
            let (read, write) = os::make_pipe()?;
            let (parent_end, child_end) = if parent_writes {
                (write, read)
            } else {
                (read, write)
            };
            os::set_inheritable(&parent_end, false)?;
            *parent_ref = Some(parent_end);
            *child_ref = Some(Arc::new(child_end));
            Ok(())
        }
        fn prepare_file(file: File, child_ref: &mut Option<Arc<File>>) -> io::Result<()> {
            // Make the File inheritable and store it for use in the child.
            os::set_inheritable(&file, true)?;
            *child_ref = Some(Arc::new(file));
            Ok(())
        }
        fn prepare_shared_file(
            file: Arc<File>,
            child_ref: &mut Option<Arc<File>>,
        ) -> io::Result<()> {
            // Like prepare_file, but for Arc<File>
            os::set_inheritable(&file, true)?;
            *child_ref = Some(file);
            Ok(())
        }
        fn reuse_stream(
            dest: &mut Option<Arc<File>>,
            src: &mut Option<Arc<File>>,
            src_id: StandardStream,
        ) -> io::Result<()> {
            // For Redirection::Merge, make stdout and stderr refer to the same File.  If
            // the file is unavailable, use the appropriate system output stream.
            if src.is_none() {
                *src = Some(get_standard_stream(src_id)?);
            }
            *dest = src.clone();
            Ok(())
        }

        #[derive(PartialEq, Eq, Copy, Clone)]
        enum MergeKind {
            ErrToOut, // 2>&1
            OutToErr, // 1>&2
            None,
        }
        let mut merge: MergeKind = MergeKind::None;

        let (mut child_stdin, mut child_stdout, mut child_stderr) = (None, None, None);

        match stdin {
            Redirection::Pipe => prepare_pipe(true, &mut self.stdin, &mut child_stdin)?,
            Redirection::File(file) => prepare_file(file, &mut child_stdin)?,
            Redirection::SharedFile(file) => prepare_shared_file(file, &mut child_stdin)?,
            Redirection::Merge => {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    "Redirection::Merge not valid for stdin",
                ));
            }
            Redirection::None => (),
        };
        match stdout {
            Redirection::Pipe => prepare_pipe(false, &mut self.stdout, &mut child_stdout)?,
            Redirection::File(file) => prepare_file(file, &mut child_stdout)?,
            Redirection::SharedFile(file) => prepare_shared_file(file, &mut child_stdout)?,
            Redirection::Merge => merge = MergeKind::OutToErr,
            Redirection::None => (),
        };
        match stderr {
            Redirection::Pipe => prepare_pipe(false, &mut self.stderr, &mut child_stderr)?,
            Redirection::File(file) => prepare_file(file, &mut child_stderr)?,
            Redirection::SharedFile(file) => prepare_shared_file(file, &mut child_stderr)?,
            Redirection::Merge => {
                if merge != MergeKind::None {
                    return Err(io::Error::new(
                        ErrorKind::InvalidInput,
                        "Redirection::Merge not valid for both stdout and stderr",
                    ));
                }
                merge = MergeKind::ErrToOut;
            }
            Redirection::None => (),
        };

        // Handle Redirection::Merge after creating the output child streams.  Merge by
        // cloning the child stream, or the appropriate standard stream if we don't have a
        // child stream requested using Redirection::Pipe or Redirection::File.  In other
        // words, 2>&1 (ErrToOut) is implemented by making child_stderr point to a dup of
        // child_stdout, or of the OS's stdout stream.
        match merge {
            MergeKind::ErrToOut => {
                reuse_stream(&mut child_stderr, &mut child_stdout, StandardStream::Output)?
            }
            MergeKind::OutToErr => {
                reuse_stream(&mut child_stdout, &mut child_stderr, StandardStream::Error)?
            }
            MergeKind::None => (),
        }

        Ok((child_stdin, child_stdout, child_stderr))
    }

    /// Mark the process as detached.
    ///
    /// This method has no effect on the OS level, it simply tells `Popen` not to wait for the
    /// subprocess to finish when going out of scope.  If the child process has already
    /// finished, or if it is guaranteed to finish before `Popen` goes out of scope, calling
    /// `detach` has no effect.
    pub fn detach(&mut self) {
        self.detached = true;
    }

    /// Return the PID of the subprocess, if it is known to be still running.
    ///
    /// Note that this method won't actually *check* whether the child process is still
    /// running, it will only return the information last set using one of `create`, `wait`,
    /// `wait_timeout`, or `poll`.  For a newly created `Popen`, `pid()` always returns
    /// `Some`.
    pub fn pid(&self) -> Option<u32> {
        match self.child_state {
            Running { pid, .. } => Some(pid),
            _ => None,
        }
    }

    /// Return the exit status of the subprocess, if it is known to have finished.
    ///
    /// Note that this method won't actually *check* whether the child process has finished,
    /// it only returns the previously available information.  To check or wait for the
    /// process to finish, call `wait`, `wait_timeout`, or `poll`.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        match self.child_state {
            Finished(exit_status) => Some(exit_status),
            _ => None,
        }
    }

    /// Prepare to send input to the subprocess and capture its output.
    ///
    /// Sets up writing `input_data` to the subprocess's stdin (then closing it) while
    /// simultaneously reading stdout and stderr until end-of-file.  The actual I/O is
    /// deferred until you call [`read`] or [`read_string`] on the returned [`Communicator`].
    ///
    /// The simultaneous reading and writing avoids deadlock when the subprocess produces
    /// output before consuming all input.  (A naive write-then-read approach would hang
    /// because the parent blocks on writing while the child blocks on having its output read.)
    ///
    /// Unlike [`communicate_bytes`], the `Communicator` allows timeout, size limits, and
    /// access to partial output on error.
    ///
    /// [`Communicator`]: struct.Communicator.html
    /// [`read`]: struct.Communicator.html#method.read
    /// [`read_string`]: struct.Communicator.html#method.read_string
    /// [`communicate_bytes`]: #method.communicate_bytes
    pub fn communicate_start(&mut self, input_data: Option<Vec<u8>>) -> Communicator {
        communicate::communicate(
            self.stdin.take(),
            self.stdout.take(),
            self.stderr.take(),
            input_data,
        )
    }

    /// Send input to the subprocess and capture its output.
    ///
    /// Writes `input_data` to the subprocess's stdin and closes it, while simultaneously
    /// reading stdout and stderr until end-of-file.  Returns the captured output as a pair of
    /// `Option<Vec<u8>>`, where `None` indicates a stream not redirected to `Pipe`.
    ///
    /// The simultaneous reading and writing avoids deadlock when the subprocess produces
    /// output before consuming all input.
    ///
    /// This method does not wait for the subprocess to exit, only for its output streams to
    /// reach EOF.  In rare cases where a process continues after closing its streams,
    /// [`Popen::drop`] will wait for it.  Use [`wait`], [`detach`], or [`terminate`] if you
    /// need explicit control.
    ///
    /// For timeout and size limit support, use [`communicate_start`] instead.
    ///
    /// # Panics
    ///
    /// If `input_data` is provided and `stdin` was not redirected to a pipe.  Also, if
    /// `input_data` is not provided and `stdin` was redirected to a pipe.
    ///
    /// # Errors
    ///
    /// * `Err(::std::io::Error)` if a system call fails
    ///
    /// [`wait`]: #method.wait
    /// [`detach`]: #method.detach
    /// [`terminate`]: #method.terminate
    /// [`communicate_start`]: #method.communicate_start
    pub fn communicate_bytes(
        &mut self,
        input_data: Option<&[u8]>,
    ) -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        self.communicate_start(input_data.map(|i| i.to_vec()))
            .read()
            .map_err(|e| e.error)
    }

    /// Feed the subprocess with data and capture its output as string.
    ///
    /// This is a convenience method equivalent to [`communicate_bytes`], but with input as
    /// `&str` and output as `String`.  Invalid UTF-8 sequences, if found, are replaced with
    /// the `U+FFFD` Unicode replacement character.
    ///
    /// # Panics
    ///
    /// The same as with `communicate_bytes`.
    ///
    /// # Errors
    ///
    /// * `Err(::std::io::Error)` if a system call fails
    ///
    /// [`communicate_bytes`]: struct.Popen.html#method.communicate_bytes
    pub fn communicate(
        &mut self,
        input_data: Option<&str>,
    ) -> io::Result<(Option<String>, Option<String>)> {
        self.communicate_start(input_data.map(|s| s.as_bytes().to_vec()))
            .read_string()
            .map_err(|e| e.error)
    }

    /// Check whether the process is still running, without blocking or errors.
    ///
    /// This checks whether the process is still running and if it is still running, `None` is
    /// returned, otherwise `Some(exit_status)`.  This method is guaranteed not to block and
    /// is exactly equivalent to `wait_timeout(Duration::from_secs(0)).unwrap_or(None)`.
    pub fn poll(&mut self) -> Option<ExitStatus> {
        self.wait_timeout(Duration::from_secs(0)).unwrap_or(None)
    }

    /// Wait for the process to finish, and return its exit status.
    ///
    /// If the process has already finished, it will exit immediately, returning the exit
    /// status.  Calling `wait` after that will return the cached exit status without
    /// executing any system calls.
    ///
    /// # Errors
    ///
    /// Returns an `Err` if a system call fails in an unpredicted way.
    /// This should not happen in normal usage.
    pub fn wait(&mut self) -> Result<ExitStatus> {
        self.os_wait()
    }

    /// Wait for the process to finish, timing out after the specified duration.
    ///
    /// This function behaves like `wait()`, except that the caller will be blocked for
    /// roughly no longer than `dur`.  It returns `Ok(None)` if the timeout is known to have
    /// elapsed.
    ///
    /// On Unix-like systems, timeout is implemented by calling `waitpid(..., WNOHANG)` in a
    /// loop with adaptive sleep intervals between iterations.
    pub fn wait_timeout(&mut self, dur: Duration) -> Result<Option<ExitStatus>> {
        self.os_wait_timeout(dur)
    }

    /// Terminate the subprocess.
    ///
    /// On Unix-like systems, this sends the `SIGTERM` signal to the child process, which can
    /// be caught by the child in order to perform cleanup before exiting.  On Windows, it is
    /// equivalent to `kill()`.
    pub fn terminate(&mut self) -> io::Result<()> {
        self.os_terminate()
    }

    /// Kill the subprocess.
    ///
    /// On Unix-like systems, this sends the `SIGKILL` signal to the child process, which
    /// cannot be caught.
    ///
    /// On Windows, it invokes [`TerminateProcess`] on the process handle with equivalent
    /// semantics.
    ///
    /// [`TerminateProcess`]: https://msdn.microsoft.com/en-us/library/windows/desktop/ms686714(v=vs.85).aspx
    pub fn kill(&mut self) -> io::Result<()> {
        self.os_kill()
    }
}

trait PopenOs {
    fn os_start(&mut self, argv: Vec<OsString>, config: PopenConfig) -> Result<()>;
    fn os_wait(&mut self) -> Result<ExitStatus>;
    fn os_wait_timeout(&mut self, dur: Duration) -> Result<Option<ExitStatus>>;
    fn os_terminate(&mut self) -> io::Result<()>;
    fn os_kill(&mut self) -> io::Result<()>;
}

#[cfg(unix)]
mod os {
    use super::*;

    use crate::posix;
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};

    use crate::os_common::ExitStatus;
    use crate::unix::PopenExt;

    /// Read exactly N bytes, or return None on immediate EOF. Similar to read_exact(),
    /// but distinguishes between no read and partial read (which is treated as error).
    fn read_exact_or_eof<const N: usize>(source: &mut File) -> io::Result<Option<[u8; N]>> {
        let mut buf = [0u8; N];
        let mut total_read = 0;
        while total_read < N {
            let n = source.read(&mut buf[total_read..])?;
            if n == 0 {
                break;
            }
            total_read += n;
        }
        match total_read {
            0 => Ok(None),
            n if n == N => Ok(Some(buf)),
            _ => Err(io::ErrorKind::UnexpectedEof.into()),
        }
    }

    pub type ExtChildState = ();

    impl super::PopenOs for Popen {
        fn os_start(&mut self, argv: Vec<OsString>, config: PopenConfig) -> Result<()> {
            let mut exec_fail_pipe = posix::pipe()?;
            set_inheritable(&exec_fail_pipe.0, false)?;
            set_inheritable(&exec_fail_pipe.1, false)?;
            {
                let child_ends = self.setup_streams(config.stdin, config.stdout, config.stderr)?;
                let child_env = config.env.as_deref().map(format_env);
                let cmd_to_exec = config.executable.as_ref().unwrap_or(&argv[0]);
                let just_exec = posix::prep_exec(cmd_to_exec, &argv, child_env.as_deref())?;
                unsafe {
                    // unsafe because after the call to fork() the child is not allowed to
                    // allocate
                    match posix::fork()? {
                        Some(child_pid) => {
                            self.child_state = Running {
                                pid: child_pid,
                                ext: (),
                            };
                        }
                        None => {
                            drop(exec_fail_pipe.0);
                            let result = do_exec(
                                just_exec,
                                child_ends,
                                config.cwd.as_deref(),
                                config.setuid,
                                config.setgid,
                                config.setpgid,
                            );
                            // If we are here, it means that exec has failed.  Notify the
                            // parent and exit.
                            let error_code = match result {
                                Ok(()) => unreachable!(),
                                Err(e) => e.raw_os_error().unwrap_or(-1),
                            } as u32;
                            exec_fail_pipe.1.write_all(&error_code.to_le_bytes()).ok();
                            posix::_exit(127);
                        }
                    }
                }
            }
            drop(exec_fail_pipe.1);
            match read_exact_or_eof::<4>(&mut exec_fail_pipe.0)? {
                None => Ok(()),
                Some(error_buf) => {
                    let error_code = u32::from_le_bytes(error_buf);
                    Err(io::Error::from_raw_os_error(error_code as i32))
                }
            }
        }

        fn os_wait(&mut self) -> Result<ExitStatus> {
            while let Running { .. } = self.child_state {
                self.waitpid(true)?;
            }
            Ok(self.exit_status().unwrap())
        }

        fn os_wait_timeout(&mut self, dur: Duration) -> Result<Option<ExitStatus>> {
            use std::cmp::min;

            if let Finished(exit_status) = self.child_state {
                return Ok(Some(exit_status));
            }

            let deadline = Instant::now() + dur;
            // double delay at every iteration, maxing at 100ms
            let mut delay = Duration::from_millis(1);

            loop {
                self.waitpid(false)?;
                if let Finished(exit_status) = self.child_state {
                    return Ok(Some(exit_status));
                }
                let now = Instant::now();
                if now >= deadline {
                    return Ok(None);
                }
                let remaining = deadline.duration_since(now);
                ::std::thread::sleep(min(delay, remaining));
                delay = min(delay * 2, Duration::from_millis(100));
            }
        }

        fn os_terminate(&mut self) -> io::Result<()> {
            self.send_signal(posix::SIGTERM)
        }

        fn os_kill(&mut self) -> io::Result<()> {
            self.send_signal(posix::SIGKILL)
        }
    }

    fn format_env(env: &[(OsString, OsString)]) -> Vec<OsString> {
        // Convert Vec of (key, val) pairs to Vec of key=val, as required by execvpe.
        // Eliminate dups, in favor of later-appearing entries.
        let mut seen = HashSet::<&OsStr>::new();
        let mut formatted: Vec<_> = env
            .iter()
            .rev()
            .filter(|&(k, _)| seen.insert(k))
            .map(|(k, v)| {
                let mut fmt = k.clone();
                fmt.push("=");
                fmt.push(v);
                fmt
            })
            .collect();
        formatted.reverse();
        formatted
    }

    fn dup2_if_needed(file: Option<Arc<File>>, target_fd: i32) -> io::Result<()> {
        if let Some(f) = file
            && f.as_raw_fd() != target_fd
        {
            posix::dup2(f.as_raw_fd(), target_fd)?;
        }
        Ok(())
    }

    fn do_exec(
        just_exec: impl FnOnce() -> io::Result<()>,
        child_ends: (Option<Arc<File>>, Option<Arc<File>>, Option<Arc<File>>),
        cwd: Option<&OsStr>,
        setuid: Option<u32>,
        setgid: Option<u32>,
        setpgid: bool,
    ) -> io::Result<()> {
        if let Some(cwd) = cwd {
            env::set_current_dir(cwd)?;
        }

        let (stdin, stdout, stderr) = child_ends;
        dup2_if_needed(stdin, 0)?;
        dup2_if_needed(stdout, 1)?;
        dup2_if_needed(stderr, 2)?;
        posix::reset_sigpipe()?;

        // setgid must come before setuid: once we drop privileges with setuid, we may
        // no longer have permission to call setgid
        if let Some(gid) = setgid {
            posix::setgid(gid)?;
        }
        if let Some(uid) = setuid {
            posix::setuid(uid)?;
        }
        if setpgid {
            posix::setpgid(0, 0)?;
        }
        just_exec()?;
        unreachable!();
    }

    impl Popen {
        fn waitpid(&mut self, block: bool) -> io::Result<()> {
            let pid = match self.child_state {
                Preparing => panic!("child_state == Preparing"),
                Running { pid, .. } => pid,
                Finished(..) => return Ok(()),
            };
            match posix::waitpid(pid, if block { 0 } else { posix::WNOHANG }) {
                Ok((pid_out, exit_status)) if pid_out == pid => {
                    self.child_state = Finished(exit_status);
                }
                Ok(_) => {}
                Err(e) if e.raw_os_error() == Some(posix::ECHILD) => {
                    // Someone else has waited for the child (another thread, a signal
                    // handler...). The PID no longer exists and we cannot find its exit status.
                    self.child_state = Finished(ExitStatus::Undetermined);
                }
                Err(e) => return Err(e),
            }
            Ok(())
        }
    }

    pub fn set_inheritable(f: &File, inheritable: bool) -> io::Result<()> {
        // Unix pipes are inheritable by default, so we only need to act when removing the flag.
        if !inheritable {
            let fd = f.as_raw_fd();
            let old = posix::fcntl(fd, posix::F_GETFD, None)?;
            posix::fcntl(fd, posix::F_SETFD, Some(old | posix::FD_CLOEXEC))?;
        }
        Ok(())
    }

    /// Create a pipe.
    ///
    /// This is a safe wrapper over `libc::pipe`.
    pub fn make_pipe() -> io::Result<(File, File)> {
        posix::pipe()
    }

    pub mod ext {
        use crate::popen::ChildState::*;
        use crate::popen::Popen;
        use crate::posix;
        use std::io;

        /// Unix-specific extension methods for `Popen`
        pub trait PopenExt {
            /// Send the specified signal to the child process.
            ///
            /// The signal numbers are best obtained from the [`libc`] crate.
            ///
            /// If the child process is known to have finished (due to e.g. a previous call to
            /// [`wait`] or [`poll`]), this will do nothing and return `Ok`.
            ///
            /// [`poll`]: ../struct.Popen.html#method.poll
            /// [`wait`]: ../struct.Popen.html#method.wait
            /// [`libc`]: https://docs.rs/libc/
            fn send_signal(&self, signal: i32) -> io::Result<()>;

            /// Send the specified signal to the child's process group.
            ///
            /// This is useful for terminating a tree of processes spawned by the child.
            /// For this to work correctly, the child should be started with
            /// [`PopenConfig::setpgid`] set to `true`, which places the child
            /// in a new process group with PGID equal to its PID.
            ///
            /// [`PopenConfig::setpgid`]: crate::PopenConfig#structfield.setpgid
            ///
            /// If the child process is known to have finished, this will do nothing
            /// and return `Ok`.
            fn send_signal_group(&self, signal: i32) -> io::Result<()>;
        }
        impl PopenExt for Popen {
            fn send_signal(&self, signal: i32) -> io::Result<()> {
                match self.child_state {
                    Preparing => panic!("child_state == Preparing"),
                    Running { pid, .. } => posix::kill(pid, signal),
                    Finished(..) => Ok(()),
                }
            }

            fn send_signal_group(&self, signal: i32) -> io::Result<()> {
                match self.child_state {
                    Preparing => panic!("child_state == Preparing"),
                    Running { pid, .. } => posix::killpg(pid, signal),
                    Finished(..) => Ok(()),
                }
            }
        }
    }
}

#[cfg(windows)]
mod os {
    use super::*;

    use std::collections::HashSet;
    use std::env;
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{AsRawHandle, RawHandle};
    use std::time::Duration;

    use crate::os_common::{ExitStatus, StandardStream};
    use crate::win32;

    #[derive(Debug)]
    pub struct ExtChildState(win32::Handle);

    impl super::PopenOs for Popen {
        fn os_start(&mut self, argv: Vec<OsString>, config: PopenConfig) -> Result<()> {
            fn raw(opt: Option<&Arc<File>>) -> Option<RawHandle> {
                opt.map(|f| f.as_raw_handle())
            }
            let (mut child_stdin, mut child_stdout, mut child_stderr) =
                self.setup_streams(config.stdin, config.stdout, config.stderr)?;
            ensure_child_stream(&mut child_stdin, StandardStream::Input)?;
            ensure_child_stream(&mut child_stdout, StandardStream::Output)?;
            ensure_child_stream(&mut child_stderr, StandardStream::Error)?;
            let cmdline = assemble_cmdline(argv)?;
            let env_block = config.env.map(|env| format_env_block(&env));
            // CreateProcess doesn't search for appname in the PATH. We do it ourselves to
            // match the Unix behavior.
            let executable = config.executable.map(locate_in_path);
            let (handle, pid) = win32::CreateProcess(
                executable.as_ref().map(OsString::as_ref),
                &cmdline,
                env_block.as_deref(),
                config.cwd.as_deref(),
                true,
                config.creation_flags,
                raw(child_stdin.as_ref()),
                raw(child_stdout.as_ref()),
                raw(child_stderr.as_ref()),
                win32::STARTF_USESTDHANDLES,
            )?;
            self.child_state = Running {
                pid: pid as u32,
                ext: ExtChildState(handle),
            };
            Ok(())
        }

        fn os_wait(&mut self) -> Result<ExitStatus> {
            self.wait_handle(None)?;
            // wait_handle(None) should always result in Finished state. The only way for it
            // not to would be if WaitForSingleObject returned something other than OBJECT_0.
            self.exit_status().ok_or_else(|| {
                io::Error::other("os_wait: child state is not Finished after WaitForSingleObject")
            })
        }

        fn os_wait_timeout(&mut self, dur: Duration) -> Result<Option<ExitStatus>> {
            if let Finished(exit_status) = self.child_state {
                return Ok(Some(exit_status));
            }
            self.wait_handle(Some(dur))?;
            Ok(self.exit_status())
        }

        fn os_terminate(&mut self) -> io::Result<()> {
            let mut new_child_state = None;
            if let Running {
                ext: ExtChildState(ref handle),
                ..
            } = self.child_state
                && let Err(err) = win32::TerminateProcess(handle, 1)
            {
                if err.raw_os_error() != Some(win32::ERROR_ACCESS_DENIED as i32) {
                    return Err(err);
                }
                let rc = win32::GetExitCodeProcess(handle)?;
                if rc == win32::STILL_ACTIVE {
                    return Err(err);
                }
                new_child_state = Some(Finished(ExitStatus::Exited(rc)));
            }
            if let Some(new_child_state) = new_child_state {
                self.child_state = new_child_state;
            }
            Ok(())
        }

        fn os_kill(&mut self) -> io::Result<()> {
            self.terminate()
        }
    }

    fn format_env_block(env: &[(OsString, OsString)]) -> Vec<u16> {
        fn to_uppercase(s: &OsStr) -> OsString {
            OsString::from_wide(
                &s.encode_wide()
                    .map(|c| {
                        if c < 128 {
                            (c as u8).to_ascii_uppercase() as u16
                        } else {
                            c
                        }
                    })
                    .collect::<Vec<_>>(),
            )
        }
        let mut pruned: Vec<_> = {
            let mut seen = HashSet::<OsString>::new();
            env.iter()
                .rev()
                .filter(|&(k, _)| seen.insert(to_uppercase(k)))
                .collect()
        };
        pruned.reverse();
        let mut block = vec![];
        for (k, v) in pruned {
            block.extend(k.encode_wide());
            block.push('=' as u16);
            block.extend(v.encode_wide());
            block.push(0);
        }
        block.push(0);
        block
    }

    impl Popen {
        fn wait_handle(&mut self, timeout: Option<Duration>) -> io::Result<()> {
            let mut new_child_state = None;
            if let Running {
                ext: ExtChildState(ref handle),
                ..
            } = self.child_state
            {
                let event = win32::WaitForSingleObject(handle, timeout)?;
                if let win32::WaitEvent::OBJECT_0 = event {
                    let exit_code = win32::GetExitCodeProcess(handle)?;
                    new_child_state = Some(Finished(ExitStatus::Exited(exit_code)));
                }
            }
            if let Some(new_child_state) = new_child_state {
                self.child_state = new_child_state;
            }
            Ok(())
        }
    }

    fn ensure_child_stream(
        stream: &mut Option<Arc<File>>,
        which: StandardStream,
    ) -> io::Result<()> {
        // If no stream is sent to CreateProcess, the child doesn't get a valid stream.
        // This results in e.g. Exec("sh").arg("-c").arg("echo foo >&2").stream_stderr()
        // failing because the shell tries to redirect stdout to stderr, but fails because
        // it didn't receive a valid stdout.
        if stream.is_none() {
            *stream = Some(get_standard_stream(which)?);
        }
        Ok(())
    }

    pub fn set_inheritable(f: &File, inheritable: bool) -> io::Result<()> {
        win32::SetHandleInformation(
            f,
            win32::HANDLE_FLAG_INHERIT,
            if inheritable { 1 } else { 0 },
        )?;
        Ok(())
    }

    /// Create a pipe where both ends support overlapped I/O.
    ///
    /// Both handles are created inheritable; callers should use `set_inheritable`
    /// to make the parent's end non-inheritable before spawning children.
    pub fn make_pipe() -> io::Result<(File, File)> {
        // We create overlap pipes because Windows `communicate()` requires overlapped
        // (async) I/O to simultaneously read from stdout/stderr and write to stdin
        // without deadlocking. This is analogous to how Unix uses `poll()` for the same
        // purpose.
        //
        // Although MSDN warns that passing NULL for lpOverlapped on an overlapped
        // handle "can incorrectly report that the read operation is complete", in
        // practice synchronous I/O works correctly on overlapped pipe handles.
        //
        // Raymond Chen notes that for pipes and mailslots, the I/O subsystem
        // accepts synchronous I/O on overlapped handles
        // https://devblogs.microsoft.com/oldnewthing/20120411-00/?p=7883
        //
        // This means both `communicate()` (which uses overlapped I/O) and the
        // `stream_*` methods (which use synchronous File::read/write) work correctly
        // with these pipes.
        win32::make_pipe()
    }

    fn locate_in_path(executable: OsString) -> OsString {
        let Some(path_var) = env::var_os("PATH") else {
            return executable;
        };
        for dir in env::split_paths(&path_var) {
            let candidate = dir
                .join(&executable)
                .with_extension(std::env::consts::EXE_EXTENSION);
            if candidate.exists() {
                return candidate.into_os_string();
            }
        }
        executable
    }

    fn assemble_cmdline(argv: Vec<OsString>) -> io::Result<OsString> {
        let mut cmdline = vec![];
        for (i, arg) in argv.iter().enumerate() {
            if i > 0 {
                cmdline.push(' ' as u16);
            }
            if arg.encode_wide().any(|c| c == 0) {
                return Err(io::Error::from_raw_os_error(win32::ERROR_BAD_PATHNAME as _));
            }
            append_quoted(arg, &mut cmdline);
        }
        Ok(OsString::from_wide(&cmdline))
    }

    // Translated from ArgvQuote at http://tinyurl.com/zmgtnls
    fn append_quoted(arg: &OsStr, cmdline: &mut Vec<u16>) {
        if !arg.is_empty()
            && !arg.encode_wide().any(|c| {
                c == ' ' as u16
                    || c == '\t' as u16
                    || c == '\n' as u16
                    || c == '\x0b' as u16
                    || c == '\"' as u16
            })
        {
            cmdline.extend(arg.encode_wide());
            return;
        }
        cmdline.push('"' as u16);

        let arg: Vec<_> = arg.encode_wide().collect();
        let mut i = 0;
        while i < arg.len() {
            let mut num_backslashes = 0;
            while i < arg.len() && arg[i] == '\\' as u16 {
                i += 1;
                num_backslashes += 1;
            }

            if i == arg.len() {
                for _ in 0..num_backslashes * 2 {
                    cmdline.push('\\' as u16);
                }
                break;
            } else if arg[i] == b'"' as u16 {
                for _ in 0..num_backslashes * 2 + 1 {
                    cmdline.push('\\' as u16);
                }
                cmdline.push(arg[i]);
            } else {
                for _ in 0..num_backslashes {
                    cmdline.push('\\' as u16);
                }
                cmdline.push(arg[i]);
            }
            i += 1;
        }
        cmdline.push('"' as u16);
    }

    pub mod ext {}
}

impl Drop for Popen {
    // Wait for the process to exit.  To avoid the wait, call detach().
    fn drop(&mut self) {
        // Close stdin before waiting for the child to exit. This prevents deadlock by
        // delivering EOF in case the child reads from stdin before exiting.
        self.stdin = None;
        if let (false, &Running { .. }) = (self.detached, &self.child_state) {
            // Should we log error if one occurs during drop()?
            self.wait().ok();
        }
    }
}

#[cfg(unix)]
use crate::posix::make_standard_stream;
#[cfg(windows)]
use crate::win32::make_standard_stream;

fn get_standard_stream(which: StandardStream) -> io::Result<Arc<File>> {
    static STREAMS: [OnceLock<Arc<File>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let lock = &STREAMS[which as usize];
    if let Some(stream) = lock.get() {
        return Ok(Arc::clone(stream));
    }
    let stream = make_standard_stream(which)?;
    // in case of another thread getting here first, our `stream` will just be dropped.
    // That can happen at most once.
    Ok(Arc::clone(lock.get_or_init(|| stream)))
}

/// Result type for operations in the `subprocess` crate.
pub type Result<T> = io::Result<T>;
