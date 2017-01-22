use std::result;
use std::error::Error;
use std::io;
use std::io::Result as IoResult;
use std::fs::File;
use std::string::FromUtf8Error;
use std::fmt;
use std::ffi::{OsStr, OsString};
use std::time::Duration;

use os_common::{ExitStatus, StandardStream};
use communicate;

use self::ChildState::*;

pub use self::os::ext as os_ext;

/// Interface to a running subprocess.
///
/// `Popen` is the parent's interface to a running subprocess.  The
/// child process is started in the constructor, so owning `Popen`
/// means that the program has been successfully executed.  To prevent
/// accumulation of zombie processes, the child is automatically
/// waited upon when a `Popen` goes out of scope, which can be
/// prevented using the [`detach`] method.
///
/// Depending on how the subprocess was configured, its input, output,
/// and error streams can be connected to the parent and available as
/// [`stdin`], [`stdout`], and [`stderr`] public fields.  To simply
/// read the output and errors into memory (and optionally provide
/// input the same way), use the [`communicate_bytes`] or
/// [`communicate`] methods, which guarantee deadlock-free
/// communication with the subprocess.
///
/// `Popen` instances can be obtained with the [`create`] method, or
/// using the [`popen`] method of the [`Exec`] class.  Subprocesses
/// can be connected into pipes, most easily achieved using using
/// [`Exec`].
///
/// [`Exec`]: struct.Exec.html
/// [`stdin`]: struct.Popen.html#structfield.stdin
/// [`stdout`]: struct.Popen.html#structfield.stdout
/// [`stderr`]: struct.Popen.html#structfield.stderr
/// [`create`]: struct.Popen.html#method.create
/// [`communicate`]: struct.Popen.html#method.communicate
/// [`communicate_bytes`]: struct.Popen.html#method.communicate_bytes
/// [`detach`]: struct.Popen.html#method.detach

#[derive(Debug)]
pub struct Popen {
    /// If `stdin` was specified as `Redirection::Pipe`, this will
    /// contain a writeble `File` connected to the standard input of
    /// the child process.
    pub stdin: Option<File>,

    /// If `stdout` was specified as `Redirection::Pipe`, this will
    /// contain a readable `File` connected to the standard output of
    /// the child process.
    pub stdout: Option<File>,

    /// If `stderr` was specified as `Redirection::Pipe`, this will
    /// contain a readable `File` connected to the standard error of
    /// the child process.
    pub stderr: Option<File>,

    child_state: ChildState,
    detached: bool,
}

#[derive(Debug)]
enum ChildState {
    Preparing,                  // only during construction
    Running {
        pid: u32,
        ext: os::ExtChildState,
    },
    Finished(ExitStatus),
}

mod fileref {
    // FileRef: a reference-counted File instance, allowing multiple
    // references to the same File.  If the underlying File is Owned,
    // it will be closed along with the last FileRef.  If unowned
    // (used for system streams), it will remain open.

    use std::fs::File;
    use os_common::Undropped;
    use std::rc::Rc;
    use std::ops::Deref;

    #[derive(Debug)]
    enum InnerFile {
        Owned(File),
        System(Undropped<File>),
    }

    #[derive(Debug, Clone)]
    pub struct FileRef(Rc<InnerFile>);

    impl FileRef {
        pub fn from_owned(f: File) -> FileRef {
            FileRef(Rc::new(InnerFile::Owned(f)))
        }
        pub fn from_system(f: Undropped<File>) -> FileRef {
            FileRef(Rc::new(InnerFile::System(f)))
        }
    }

    impl Deref for FileRef {
        type Target = File;

        fn deref(&self) -> &File {
            match *self.0.deref() {
                InnerFile::Owned(ref f) => f,
                InnerFile::System(ref f) => f.get_ref(),
            }
        }
    }
}
use self::fileref::FileRef;

/// Options for [`Popen::create`].
///
/// When constructing `PopenConfig`, always use the [`Default`] trait,
/// such as:
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
/// An alternative to using `PopenConfig` directly is creating
/// processes using [`Exec`], a builder for `Popen`.
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

    /// Executable to run.
    ///
    /// If provided, this executable will be used to run the program
    /// instead of `argv[0]`.  However, `argv[0]` will still be passed
    /// to the subprocess, which will see that as `argv[0]`.  On some
    /// Unix systems, `ps` will show the string passed as `argv[0]`,
    /// even though `executable` is actually running.
    pub executable: Option<OsString>,

    // force construction using ..Default::default()
    #[doc(hidden)]
    pub _use_default_to_construct: (),

    // cwd, env, preexec_fn, close_fds...
}

impl PopenConfig {
    /// Clone the underlying [`PopenConfig`], or return an error.
    ///
    /// This is guaranteed not to fail as long as no
    /// [`Redirection::File`] variant is used for one of the standard
    /// streams.  Otherwise, it fails if `File::try_clone` fails on
    /// one of the `Redirection`s.
    ///
    /// [`PopenConfig`]: struct.PopenConfig.html
    /// [`Redirection::File`]: enum.Redirection.html#variant.File
    pub fn try_clone(&self) -> IoResult<PopenConfig> {
        Ok(PopenConfig {
            _use_default_to_construct: (),
            stdin: self.stdin.try_clone()?,
            stdout: self.stdout.try_clone()?,
            stderr: self.stderr.try_clone()?,
            detached: self.detached,
            executable: self.executable.as_ref().cloned(),
        })
    }
}

impl Default for PopenConfig {
    fn default() -> PopenConfig {
        PopenConfig {
            _use_default_to_construct: (),
            stdin: Redirection::None,
            stdout: Redirection::None,
            stderr: Redirection::None,
            detached: false,
            executable: None,
        }
    }
}

/// Instruction what to do with a stream in the child process.
///
/// `Redirection` values are used for the `stdin`, `stdout`, and
/// `stderr` field of the `PopenConfig` struct.  They tell
/// `Popen::create` how to set up the standard streams in the child
/// process and the corresponding fields of the `Popen` struct in the
/// parent.

#[derive(Debug)]
pub enum Redirection {
    /// Do nothing with the stream.
    ///
    /// The stream is typically inherited from the parent.  The field
    /// in `Popen` corresponding to the stream will be `None`.
    None,

    /// Redirect the stream to the specified open `File`.
    ///
    /// This does not create a pipe, it simply spawns the child so
    /// that the specified stream sees that file.  The child can read
    /// from or write to the provided file on its own, without any
    /// intervention by the parent.
    ///
    /// The field in `Popen` corresponding to the stream will be
    /// `None`.
    File(File),

    /// Redirect the stream to a pipe.
    ///
    /// This creates a unidirectional pipe for the standard stream.
    /// One end is passed to the child process and configured as one
    /// of its standard streams, and the other end is available to the
    /// parent as a `File` in the corresponding field of the `Popen`
    /// struct.
    ///
    /// The field in `Popen` corresponding to the stream will be
    /// `None`.
    Pipe,

    /// Merge the stream to the other output stream.
    ///
    /// This is only valid for output streams.  Using
    /// `Redirection::Merge` for `PopenConfig::stderr` requests the
    /// child's stderr to be the same underlying file as the child's
    /// output stream (whatever that is), equivalent to the `2>&1`
    /// operator of the Bourne shell.  Analogously, using
    /// `Redirection::Merge` for `PopenConfig::stdout` is equivalent
    /// to `1>&2` in the shell.
    ///
    /// Specifying `Redirection::Merge` for `PopenConfig::stdin` or
    /// specifying it for both `stdout` and `stderr` is invalid and
    /// will cause `Popen::create` to return
    /// `Err(PopenError::LogicError)`.
    Merge,
}

impl Redirection {
    /// Clone the underlying `Redirection`, or return an error.
    ///
    /// Can fail in `File` variant.
    pub fn try_clone(&self) -> IoResult<Redirection> {
        Ok(match *self {
            Redirection::File(ref f) => Redirection::File(f.try_clone()?),
            Redirection::None => Redirection::None,
            Redirection::Pipe => Redirection::Pipe,
            Redirection::Merge => Redirection::Merge,
        })
    }
}

impl Popen {
    /// Execute an external program in a new process.
    ///
    /// `argv` is a slice containing the program followed by its
    /// arguments, such as `&["ps", "x"]`. `config` specifies details
    /// how to create and interface to the process.
    ///
    /// For example, this simply runs `cargo update`:
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
    /// If the external program cannot be executed for any reason, an
    /// error is returned.  The most typical reason for execution to
    /// fail is that the program is missing on the `PATH`, but other
    /// errors are also possible.  Note that this is distinct from the
    /// program running and then exiting with a failure code - this
    /// can be detected by calling the `wait` method to obtain its
    /// exit status.
    pub fn create<S: AsRef<OsStr>>(argv: &[S], config: PopenConfig)
                                   -> Result<Popen> {
        if argv.is_empty() {
            return Err(PopenError::LogicError("argv must not be empty"));
        }
        let argv: Vec<OsString> = argv.iter()
            .map(|p| p.as_ref().to_owned()).collect();
        let mut inst = Popen {
            stdin: None,
            stdout: None,
            stderr: None,
            child_state: ChildState::Preparing,
            detached: config.detached,
        };
        inst.os_start(argv, config.executable,
                      config.stdin, config.stdout, config.stderr)?;
        Ok(inst)
    }

    // Create the pipes requested by stdin, stdout, and stderr from
    // the PopenConfig used to construct us, and return the Files to
    // be given to the child process.
    //
    // For Redirection::Pipe, this stores the parent end of the pipe
    // to the appropriate self.std* field, and returns the child end
    // of the pipe.
    //
    // For Redirection::File, this transfers the ownership of the File
    // to the corresponding child.
    fn setup_streams(&mut self, stdin: Redirection,
                     stdout: Redirection, stderr: Redirection)
                     -> Result<(Option<FileRef>, Option<FileRef>, Option<FileRef>)> {
        fn prepare_pipe(parent_writes: bool,
                        parent_ref: &mut Option<File>, child_ref: &mut Option<FileRef>)
                        -> Result<()> {
            // Store the parent's end of the pipe into the given
            // reference, and store the child end.
            let (read, write) = os::make_pipe()?;
            let (mut parent_end, child_end) =
                if parent_writes {(write, read)} else {(read, write)};
            os::set_inheritable(&mut parent_end, false)?;
            *parent_ref = Some(parent_end);
            *child_ref = Some(FileRef::from_owned(child_end));
            Ok(())
        }
        fn prepare_file(mut file: File, child_ref: &mut Option<FileRef>)
                        -> IoResult<()> {
            // Make the File inheritable and store it for use in the child.
            os::set_inheritable(&mut file, true)?;
            *child_ref = Some(FileRef::from_owned(file));
            Ok(())
        }
        fn reuse_stream(dest: &mut Option<FileRef>, src: &mut Option<FileRef>,
                        src_id: StandardStream) -> IoResult<()> {
            // For Redirection::Merge, make stdout and stderr refer to
            // the same File.  If the file is unavailable, use the
            // appropriate system output stream.
            if src.is_none() {
                *src = Some(FileRef::from_system(os::get_standard_stream(src_id)?));
            }
            *dest = Some(src.as_ref().unwrap().clone());
            Ok(())
        }

        enum MergeKind {
            ErrToOut, // 2>&1
            OutToErr, // 1>&2
            None,
        }
        let mut merge: MergeKind = MergeKind::None;

        let (mut child_stdin, mut child_stdout, mut child_stderr)
            = (None, None, None);

        match stdin {
            Redirection::Pipe => prepare_pipe(true, &mut self.stdin,
                                              &mut child_stdin)?,
            Redirection::File(file) => prepare_file(file, &mut child_stdin)?,
            Redirection::Merge => {
                return Err(PopenError::LogicError("Redirection::Merge not valid for stdin"));
            }
            Redirection::None => (),
        };
        match stdout {
            Redirection::Pipe => prepare_pipe(false, &mut self.stdout,
                                              &mut child_stdout)?,
            Redirection::File(file) => prepare_file(file, &mut child_stdout)?,
            Redirection::Merge => merge = MergeKind::OutToErr,
            Redirection::None => (),
        };
        match stderr {
            Redirection::Pipe => prepare_pipe(false, &mut self.stderr,
                                              &mut child_stderr)?,
            Redirection::File(file) => prepare_file(file, &mut child_stderr)?,
            Redirection::Merge => merge = MergeKind::ErrToOut,
            Redirection::None => (),
        };

        // Handle Redirection::Merge after creating the output child
        // streams.  Merge by cloning the child stream, or the
        // appropriate standard stream if we don't have a child stream
        // requested using Redirection::Pipe or Redirection::File.  In
        // other words, 2>&1 (ErrToOut) is implemented by making
        // child_stderr point to a dup of child_stdout, or of the OS's
        // stdout stream.
        match merge {
            MergeKind::ErrToOut =>
                reuse_stream(&mut child_stderr, &mut child_stdout,
                             StandardStream::Output)?,
            MergeKind::OutToErr =>
                reuse_stream(&mut child_stdout, &mut child_stderr,
                             StandardStream::Error)?,
            MergeKind::None => (),
        }

        Ok((child_stdin, child_stdout, child_stderr))
    }

    /// Mark the process as detached.
    ///
    /// This method has no effect on the OS level, it simply tells
    /// `Popen` not to wait for the subprocess to finish when going
    /// out of scope.  If the child process has already finished, or
    /// if it is guaranteed to finish before `Popen` goes out of
    /// scope, calling `detach` has no effect.
    pub fn detach(&mut self) {
        self.detached = true;
    }

    /// Return the PID of the subprocess, if it is known to be still running.
    ///
    /// Note that this method won't actually *check* whether the child
    /// process is still running, it will only return the information
    /// last set using one of `create`, `wait`, `wait_timeout`, or
    /// `poll`.  For a newly created `Popen`, `pid()` always returns
    /// `Some`.
    pub fn pid(&self) -> Option<u32> {
        match self.child_state {
            Running { pid, .. } => Some(pid),
            _ => None
        }
    }

    /// Return the exit status of the subprocess, if it is known to have finished.
    ///
    /// Note that this method won't actually *check* whether the child
    /// process has finished, it only returns the previously available
    /// information.  To check or wait for the process to finish, call
    /// `wait`, `wait_timeout`, or `poll`.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        match self.child_state {
            Finished(exit_status) => Some(exit_status),
            _ => None
        }
    }

    /// Feed and capture the piped data of the subprocess.
    ///
    /// This will send the `input_data` to the subprocess, read its
    /// output and error, and return them as a pair of
    /// `Option<Vec<u8>>`.  The corresponding options will be `None` if
    /// the respective stream was not specified as `Redirection::Pipe`.
    ///
    /// The communication is guaranteed to be deadlock-free.  This is
    /// currently ensured using threads (only when interaction with
    /// more than one stream is needed), and may be converted to
    /// `poll()` in the future.
    ///
    /// Note that this method will not wait for the program to finish,
    /// only to close its output stream(s).  The program may continue
    /// running afterwards, and `wait()` must be used to ensure that
    /// it has actually finished.
    ///
    /// # Panics
    ///
    /// If `input_data` is provided and `stdin` was not redirected to
    /// a pipe.
    pub fn communicate_bytes(&mut self, input_data: Option<&[u8]>)
                             -> IoResult<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        communicate::communicate_bytes(&mut self.stdin, &mut self.stdout,
                                       &mut self.stderr, input_data)
    }

    /// Feed and capture the piped data of the subprocess as strings.
    ///
    /// This is a convenience method equivalent to
    /// `communicate_bytes`, but with input as `&str` and output as
    /// `String`.
    ///
    /// # Panics
    ///
    /// The same as with `communicate_bytes`
    ///
    /// # Errors
    ///
    /// * `Err(::std::io::Error)` if a system call fails
    /// * `Err(PopenError::Utf8Error)` if the output of the process is
    ///    not valid UTF-8 and therefore cannot be represented as a
    ///    String.
    ///
    /// # Panics
    ///
    /// If `input_data` is provided and `stdin` was not redirected to
    /// a pipe.
    pub fn communicate(&mut self, input_data: Option<&str>)
                       -> Result<(Option<String>, Option<String>)> {
        let (out, err) = self.communicate_bytes(
            input_data.map(|s| s.as_bytes()))?;
        let out_str = if let Some(out_vec) = out {
            Some(String::from_utf8(out_vec)?)
        } else { None };
        let err_str = if let Some(err_vec) = err {
            Some(String::from_utf8(err_vec)?)
        } else { None };
        Ok((out_str, err_str))
    }

    /// Check whether the process is still running, without blocking or errors.
    ///
    /// This checks whether the process is still running and.  If it
    /// is still running, `None` is returned, otherwise
    /// `Some(exit_status)`.  This method is guaranteed not to block
    /// and is exactly equivalent to
    /// `wait_timeout(Duration::new(0, 0)).unwrap_or(None)`.
    pub fn poll(&mut self) -> Option<ExitStatus> {
        self.wait_timeout(Duration::from_secs(0)).unwrap_or(None)
    }

    /// Wait for the process to finish, and return its exit status.
    ///
    /// If the process has already finished, it will exit immediately,
    /// returning the exit status.  Calling `wait` after that will
    /// return the cached exit status without executing any system
    /// calls.
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
    /// This function behaves like `wait()`, except that the caller
    /// will be blocked for roughly no longer than `dur`.  It returns
    /// `Ok(None)` if the timeout is known to have elapsed.
    ///
    /// On Unix-like systems, timeout is implemented by calling
    /// `waitpid(..., WNOHANG)` in a loop with adaptive sleep
    /// intervals between iterations.
    pub fn wait_timeout(&mut self, dur: Duration) -> Result<Option<ExitStatus>> {
        self.os_wait_timeout(dur)
    }

    /// Terminate the subprocess.
    ///
    /// On Unix-like systems, this sends the `SIGTERM` signal to the
    /// child process, which can be caught by the child in order to
    /// perform cleanup before exiting.  On Windows, it is equivalent
    /// to `kill()`.
    pub fn terminate(&mut self) -> IoResult<()> {
        self.os_terminate()
    }

    /// Kill the subprocess.
    ///
    /// On Unix-like systems, this sends the `SIGKILL` signal to the
    /// child process, which cannot be caught.
    ///
    /// On Windows, it invokes [`TerminateProcess`] on the process
    /// handle with equivalent semantics.
    ///
    /// [`TerminateProcess`]: https://msdn.microsoft.com/en-us/library/windows/desktop/ms686714(v=vs.85).aspx
    pub fn kill(&mut self) -> IoResult<()> {
        self.os_kill()
    }
}

trait PopenOs {
    fn os_start(&mut self, argv: Vec<OsString>, executable: Option<OsString>,
                stdin: Redirection, stdout: Redirection, stderr: Redirection)
                -> Result<()>;
    fn os_wait(&mut self) -> Result<ExitStatus>;
    fn os_wait_timeout(&mut self, dur: Duration) -> Result<Option<ExitStatus>>;
    fn os_terminate(&mut self) -> IoResult<()>;
    fn os_kill(&mut self) -> IoResult<()>;
}


#[cfg(unix)]
mod os {
    use super::*;
    use std::io;
    use std::io::{Read, Write, Result as IoResult};
    use std::fs::File;
    use posix;
    use std::mem;
    use std::os::unix::io::AsRawFd;
    use os_common::ExitStatus;
    use std::ffi::OsString;
    use std::time::{Duration, Instant};

    use super::ChildState::*;
    use super::fileref::FileRef;
    use unix::PopenExt;

    pub type ExtChildState = ();

    impl super::PopenOs for Popen {
        fn os_start(&mut self,
                    argv: Vec<OsString>, executable: Option<OsString>,
                    stdin: Redirection, stdout: Redirection, stderr: Redirection)
                    -> Result<()> {
            let mut exec_fail_pipe = posix::pipe()?;
            set_inheritable(&mut exec_fail_pipe.0, false)?;
            set_inheritable(&mut exec_fail_pipe.1, false)?;
            {
                let child_ends = self.setup_streams(stdin, stdout, stderr)?;
                let child_pid = posix::fork()?;
                if child_pid == 0 {
                    mem::drop(exec_fail_pipe.0);
                    let result: IoResult<()> = self.do_exec(
                        argv, executable, child_ends);
                    // If we are here, it means that exec has failed.  Notify
                    // the parent and exit.
                    let error_code = match result {
                        Ok(()) => unreachable!(),
                        Err(e) => e.raw_os_error().unwrap_or(-1)
                    } as u32;
                    exec_fail_pipe.1.write_all(
                        &[error_code as u8,
                          (error_code >> 8) as u8,
                          (error_code >> 16) as u8,
                          (error_code >> 24) as u8]).ok();
                    posix::_exit(127);
                }
                self.child_state = Running { pid: child_pid, ext: () };
            }
            mem::drop(exec_fail_pipe.1);
            let mut error_buf = [0u8; 4];
            let read_cnt = exec_fail_pipe.0.read(&mut error_buf)?;
            if read_cnt == 0 {
                Ok(())
            } else if read_cnt == 4 {
                let error_code: u32 =
                    error_buf[0] as u32 + (error_buf[1] as u32) << 8
                    + (error_buf[2] as u32) << 16 + (error_buf[3] as u32) << 24;
                Err(PopenError::from(io::Error::from_raw_os_error(error_code as i32)))
            } else {
                Err(PopenError::LogicError("invalid read_count from exec pipe"))
            }
        }

        fn os_wait(&mut self) -> Result<ExitStatus> {
            while let Running {..} = self.child_state {
                self.waitpid(true)?;
            }
            Ok(self.exit_status().unwrap())
        }

        fn os_wait_timeout(&mut self, dur: Duration)
                           -> Result<Option<ExitStatus>> {
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

        fn os_terminate(&mut self) -> IoResult<()> {
            self.send_signal(posix::SIGTERM)
        }

        fn os_kill(&mut self) -> IoResult<()> {
            self.send_signal(posix::SIGKILL)
        }
    }

    trait PopenOsImpl: super::PopenOs {
        fn do_exec(&self, argv: Vec<OsString>, executable: Option<OsString>,
                   child_ends: (Option<FileRef>, Option<FileRef>, Option<FileRef>))
                   -> IoResult<()>;
        fn waitpid(&mut self, block: bool) -> IoResult<()>;
    }

    impl PopenOsImpl for Popen {
        fn do_exec(&self, argv: Vec<OsString>, executable: Option<OsString>,
                   child_ends: (Option<FileRef>, Option<FileRef>, Option<FileRef>))
                   -> IoResult<()> {
            let (stdin, stdout, stderr) = child_ends;
            if let Some(stdin) = stdin {
                if stdin.as_raw_fd() != 0 {
                    posix::dup2(stdin.as_raw_fd(), 0)?;
                }
            }
            if let Some(stdout) = stdout {
                if stdout.as_raw_fd() != 1 {
                    posix::dup2(stdout.as_raw_fd(), 1)?;
                }
            }
            if let Some(stderr) = stderr {
                if stderr.as_raw_fd() != 2 {
                    posix::dup2(stderr.as_raw_fd(), 2)?;
                }
            }
            posix::reset_sigpipe()?;
            posix::execvp(executable.as_ref().unwrap_or(&argv[0]), &argv)
        }

        fn waitpid(&mut self, block: bool) -> IoResult<()> {
            match self.child_state {
                Preparing => panic!("child_state == Preparing"),
                Running { pid, .. } => {
                    match posix::waitpid(pid, if block { 0 }
                                         else { posix::WNOHANG }) {
                        Err(e) => {
                            if let Some(errno) = e.raw_os_error() {
                                if errno == posix::ECHILD {
                                    // Someone else has waited for the child
                                    // (another thread, a signal handler...).
                                    // The PID no longer exists and we cannot
                                    // find its exit status.
                                    self.child_state
                                        = Finished(ExitStatus::Undetermined);
                                    return Ok(());
                                }
                            }
                            return Err(e);
                        }
                        Ok((pid_out, exit_status)) => {
                            if pid_out == pid {
                                self.child_state = Finished(exit_status);
                            }
                        }
                    }
                },
                Finished(..) => (),
            }
            Ok(())
        }
    }

    pub fn set_inheritable(f: &mut File, inheritable: bool) -> IoResult<()> {
        if inheritable {
            // Unix pipes are inheritable by default.
        } else {
            let fd = f.as_raw_fd();
            let old = posix::fcntl(fd, posix::F_GETFD, None)?;
            posix::fcntl(fd, posix::F_SETFD, Some(old | posix::FD_CLOEXEC))?;
        }
        Ok(())
    }

    pub fn make_pipe() -> IoResult<(File, File)> {
        posix::pipe()
    }

    pub use posix::get_standard_stream;

    pub mod ext {
        use std::io::Result as IoResult;
        use popen::Popen;
        use popen::ChildState::*;
        use posix;

        /// Unix-specific extension methods for `Popen`
        pub trait PopenExt {
            /// Send the specified signal to the child process.
            ///
            /// The signal numbers are best obtained from the [`libc`]
            /// crate.
            ///
            /// If the child process is known to have finished (due to e.g.
            /// a previous call to [`wait`] or [`poll`]), this will do
            /// nothing and return `Ok`.
            ///
            /// [`poll`]: ../struct.Popen.html#method.poll
            /// [`wait`]: ../struct.Popen.html#method.wait
            /// [`libc`]: https://docs.rs/libc/
            fn send_signal(&self, signal: i32) -> IoResult<()>;
        }
        impl PopenExt for Popen {
            fn send_signal(&self, signal: i32) -> IoResult<()> {
                match self.child_state {
                    Preparing => panic!("child_state == Preparing"),
                    Running { pid, .. } => {
                        posix::kill(pid, signal)
                    },
                    Finished(..) => Ok(()),
                }
            }
        }
    }
}


#[cfg(windows)]
mod os {
    use super::*;
    use std::io;
    use std::fs::{self, File};
    use std::env;
    use win32;
    use os_common::{ExitStatus, StandardStream};
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{RawHandle, AsRawHandle};
    use std::time::Duration;
    use std::io::Result as IoResult;

    use super::ChildState::*;
    use super::fileref::FileRef;

    #[derive(Debug)]
    pub struct ExtChildState(win32::Handle);

    impl super::PopenOs for Popen {
        fn os_start(&mut self,
                    argv: Vec<OsString>, executable: Option<OsString>,
                    stdin: Redirection, stdout: Redirection, stderr: Redirection)
                    -> Result<()> {
            fn raw(opt: &Option<FileRef>) -> Option<RawHandle> {
                 opt.as_ref().map(|f| f.as_raw_handle())
            }
            let (mut child_stdin, mut child_stdout, mut child_stderr)
                = self.setup_streams(stdin, stdout, stderr)?;
            ensure_child_stream(&mut child_stdin, StandardStream::Input)?;
            ensure_child_stream(&mut child_stdout, StandardStream::Output)?;
            ensure_child_stream(&mut child_stderr, StandardStream::Error)?;
            let cmdline = assemble_cmdline(argv)?;
            // CreateProcess doesn't search for appname in the PATH.
            // We do it ourselves to match the Unix behavior.
            let executable = executable.map(locate_in_path);
            let (handle, pid)
                = win32::CreateProcess(executable.as_ref().map(OsString::as_ref),
                                       &cmdline, true, 0,
                                       raw(&child_stdin),
                                       raw(&child_stdout),
                                       raw(&child_stderr),
                                       win32::STARTF_USESTDHANDLES)?;
            self.child_state = Running {
                pid: pid as u32,
                ext: ExtChildState(handle)
            };
            Ok(())
        }

        fn os_wait(&mut self) -> Result<ExitStatus> {
            self.wait_handle(None)?;
            match self.child_state {
                Preparing => panic!("child_state == Preparing"),
                Finished(exit_status) => Ok(exit_status),
                // Since we invoked wait_handle without timeout, exit
                // status should exist at this point.  The only way
                // for it not to exist would be if something strange
                // happened, like WaitForSingleObject returning
                // something other than OBJECT_0.
                Running {..} => Err(
                    PopenError::LogicError("Failed to obtain exit status"))
            }
        }

        fn os_wait_timeout(&mut self, dur: Duration)
                           -> Result<Option<ExitStatus>> {
            if let Finished(exit_status) = self.child_state {
                return Ok(Some(exit_status));
            }
            self.wait_handle(Some(dur))?;
            Ok(self.exit_status())
        }

        fn os_terminate(&mut self) -> IoResult<()> {
            let mut new_child_state = None;
            if let Running { ext: ExtChildState(ref handle),
                             .. } = self.child_state {
                match win32::TerminateProcess(handle, 1) {
                    Err(err) => {
                        if err.raw_os_error() != Some(win32::ERROR_ACCESS_DENIED
                                                      as i32) {
                            return Err(err);
                        }
                        let rc = win32::GetExitCodeProcess(handle)?;
                        if rc == win32::STILL_ACTIVE {
                            return Err(err);
                        }
                        new_child_state = Some(Finished(ExitStatus::Exited(rc)));
                    }
                    Ok(_) => ()
                }
            }
            if let Some(new_child_state) = new_child_state {
                self.child_state = new_child_state;
            }
            Ok(())
        }

        fn os_kill(&mut self) -> IoResult<()> {
            self.terminate()
        }
    }

    trait PopenOsImpl: super::PopenOs {
        fn wait_handle(&mut self, timeout: Option<Duration>)
                       -> IoResult<Option<ExitStatus>>;
    }

    impl PopenOsImpl for Popen {
        fn wait_handle(&mut self, timeout: Option<Duration>)
                       -> IoResult<Option<ExitStatus>> {
            let mut new_child_state = None;
            if let Running { ext: ExtChildState(ref handle),
                             .. } = self.child_state {
                let millis = timeout.map(|t| {
                    if t <= Duration::new(4294967, 295_000_000) {
                        (t.as_secs() as u32 * 1_000
                         + t.subsec_nanos() / 1_000_000)
                    } else {
                        // Clamp to avoid overflow.  We could support timeouts
                        // longer than 49.71 days with multiple waits.
                        u32::max_value()
                    }
                });
                let event = win32::WaitForSingleObject(handle, millis)?;
                if let win32::WaitEvent::OBJECT_0 = event {
                    let exit_code = win32::GetExitCodeProcess(handle)?;
                    new_child_state = Some(Finished(ExitStatus::Exited(exit_code)));
                }
            }
            if let Some(new_child_state) = new_child_state {
                self.child_state = new_child_state;
            }
            Ok(self.exit_status())
        }
    }

    fn ensure_child_stream(stream: &mut Option<FileRef>, which: StandardStream)
                           -> IoResult<()> {
        // If no stream is sent to CreateProcess, the child doesn't
        // get a valid stream.  This results in e.g.
        // Exec("sh").arg("-c").arg("echo foo >&2").stream_stderr()
        // failing because the shell tries to redirect stdout to
        // stderr, but fails because it didn't receive a valid stdout.
        if stream.is_none() {
            *stream = Some(FileRef::from_system(get_standard_stream(which)?));
        }
        Ok(())
    }

    pub fn set_inheritable(f: &mut File, inheritable: bool) -> IoResult<()> {
        win32::SetHandleInformation(f, win32::HANDLE_FLAG_INHERIT,
                                    if inheritable {1} else {0})?;
        Ok(())
    }

    pub fn make_pipe() -> IoResult<(File, File)> {
        win32::CreatePipe(true)
    }

    fn locate_in_path(executable: OsString) -> OsString {
        if let Some(path) = env::var_os("PATH") {
            for path in env::split_paths(&path) {
                let path = path.join(&executable)
                    .with_extension(::std::env::consts::EXE_EXTENSION);
                if fs::metadata(&path).is_ok() {
                    return path.into_os_string();
                }
            }
        }
        executable
    }

    fn assemble_cmdline(argv: Vec<OsString>) -> IoResult<OsString> {
        let mut cmdline = Vec::<u16>::new();
        let mut is_first = true;
        for arg in argv {
            if !is_first {
                cmdline.push(' ' as u16);
            } else {
                is_first = false;
            }
            if arg.encode_wide().any(|c| c == 0) {
                return Err(io::Error::from_raw_os_error(
                    win32::ERROR_BAD_PATHNAME as i32));
            }
            append_quoted(&arg, &mut cmdline);
        }
        Ok(OsString::from_wide(&cmdline))
    }

    // Translated from ArgvQuote at http://tinyurl.com/zmgtnls
    fn append_quoted(arg: &OsStr, cmdline: &mut Vec<u16>) {
        if !arg.is_empty() && !arg.encode_wide().any(
            |c| c == ' ' as u16 || c == '\t' as u16 || c == '\n' as u16 ||
                c == '\x0b' as u16 || c == '\"' as u16) {
            cmdline.extend(arg.encode_wide());
            return
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
                for _ in 0..num_backslashes*2 {
                    cmdline.push('\\' as u16);
                }
                break;
            } else if arg[i] == b'"' as u16 {
                for _ in 0..num_backslashes*2 + 1 {
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

    pub use win32::get_standard_stream;

    pub mod ext {}
}


impl Drop for Popen {
    // Wait for the process to exit.  To avoid the wait, call
    // detach().
    fn drop(&mut self) {
        if let (false, &Running {..}) = (self.detached, &self.child_state) {
            // Should we log error if one occurs during drop()?
            self.wait().ok();
        }
    }
}

/// Error in [`Popen`] calls.
///
/// [`Popen`]: struct.Popen.html

#[derive(Debug)]
pub enum PopenError {
    /// Error when attempting to convert bytes to string.
    Utf8Error(FromUtf8Error),
    /// The underlying error is io::Error.
    IoError(io::Error),
    /// A logical error was made, e.g. invalid arguments detected at run-time.
    LogicError(&'static str),
}

impl From<FromUtf8Error> for PopenError {
    fn from(err: FromUtf8Error) -> PopenError {
        PopenError::Utf8Error(err)
    }
}

impl From<io::Error> for PopenError {
    fn from(err: io::Error) -> PopenError {
        PopenError::IoError(err)
    }
}

impl Error for PopenError {
    fn description(&self) -> &str {
        match *self {
            PopenError::Utf8Error(ref err) => err.description(),
            PopenError::IoError(ref err) => err.description(),
            PopenError::LogicError(description) => description,
        }
    }

    fn cause(&self) -> Option<&Error> {
        match *self {
            PopenError::Utf8Error(ref err) => Some(err as &Error),
            PopenError::IoError(ref err) => Some(err as &Error),
            PopenError::LogicError(_) => None,
        }
    }
}

impl fmt::Display for PopenError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            PopenError::Utf8Error(ref err) => fmt::Display::fmt(err, f),
            PopenError::IoError(ref err) => fmt::Display::fmt(err, f),
            PopenError::LogicError(desc) => f.write_str(desc)
        }
    }
}

/// Result returned by calls in the `subprocess` crate in places where
/// `::std::io::Result` does not suffice.
pub type Result<T> = result::Result<T, PopenError>;
