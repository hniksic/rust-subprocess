use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::io::ErrorKind;
use std::sync::{Arc, OnceLock};

#[cfg(windows)]
use crate::process::ExtProcessState;
use crate::process::Process;

pub use os::make_pipe;

/// Instruction what to do with a stream in the child process.
///
/// `Redirection` values are used for the `stdin`, `stdout`, and `stderr`
/// parameters when configuring a subprocess via [`Exec`] or [`Pipeline`].
///
/// [`Exec`]: struct.Exec.html
/// [`Pipeline`]: struct.Pipeline.html
#[derive(Debug)]
pub enum Redirection {
    /// Do nothing with the stream.
    ///
    /// The stream is typically inherited from the parent. The corresponding
    /// pipe field in [`Started`] will be `None`.
    ///
    /// [`Started`]: struct.Started.html
    None,

    /// Redirect the stream to a pipe.
    ///
    /// This variant requests that a stream be redirected to a unidirectional
    /// pipe. One end of the pipe is passed to the child process and
    /// configured as one of its standard streams, and the other end is
    /// available to the parent for communicating with the child.
    Pipe,

    /// Merge the stream to the other output stream.
    ///
    /// This variant is only valid when configuring redirection of standard
    /// output and standard error. Using `Redirection::Merge` for stderr
    /// requests the child's stderr to refer to the same underlying file as
    /// the child's stdout (which may or may not itself be redirected),
    /// equivalent to the `2>&1` operator of the Bourne shell. Analogously,
    /// using `Redirection::Merge` for stdout is equivalent to `1>&2` in the
    /// shell.
    ///
    /// Specifying `Redirection::Merge` for stdin or specifying it for both
    /// stdout and stderr is invalid and will cause an error.
    Merge,

    /// Redirect the stream to the specified open `File`.
    ///
    /// This does not create a pipe, it simply spawns the child so that the
    /// specified stream sees that file. The child can read from or write to
    /// the provided file on its own, without any intervention by the parent.
    File(File),

    /// Like `File`, but the file may be shared among multiple redirections
    /// without duplicating the file descriptor.
    SharedFile(Arc<File>),

    /// Redirect the stream to the null device (`/dev/null` on Unix, `nul`
    /// on Windows).
    ///
    /// This is equivalent to `Redirection::File` with a null device file,
    /// but more convenient and portable.
    Null,
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
            Redirection::Null => Redirection::Null,
        })
    }
}

/// Result of spawning a subprocess.
///
/// Contains the process handle and the parent ends of any pipes.
pub(crate) struct SpawnResult {
    pub process: Process,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
}

/// Spawn a subprocess.
///
/// This is the internal entry point for creating processes. It sets up stream
/// redirections, forks/creates the process, and returns the parent pipe ends along with a
/// `Process` handle.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn(
    argv: Vec<OsString>,
    stdin: Redirection,
    stdout: Redirection,
    stderr: Redirection,
    detached: bool,
    executable: Option<&OsStr>,
    env: Option<&[(OsString, OsString)]>,
    cwd: Option<&OsStr>,
    #[cfg(unix)] setuid: Option<u32>,
    #[cfg(unix)] setgid: Option<u32>,
    #[cfg(unix)] setpgid: bool,
    #[cfg(windows)] creation_flags: u32,
) -> io::Result<SpawnResult> {
    if argv.is_empty() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "argv must not be empty",
        ));
    }

    let mut parent_stdin: Option<File> = None;
    let mut parent_stdout: Option<File> = None;
    let mut parent_stderr: Option<File> = None;

    let child_ends = setup_streams(
        stdin,
        stdout,
        stderr,
        &mut parent_stdin,
        &mut parent_stdout,
        &mut parent_stderr,
    )?;

    let process = os::os_start(
        argv,
        child_ends,
        detached,
        executable,
        env,
        cwd,
        #[cfg(unix)]
        setuid,
        #[cfg(unix)]
        setgid,
        #[cfg(unix)]
        setpgid,
        #[cfg(windows)]
        creation_flags,
    )?;

    Ok(SpawnResult {
        process,
        stdin: parent_stdin,
        stdout: parent_stdout,
        stderr: parent_stderr,
    })
}

// Set up streams for the child process. Fills in the parent pipe ends and
// returns the child ends.
fn setup_streams(
    stdin: Redirection,
    stdout: Redirection,
    stderr: Redirection,
    parent_stdin: &mut Option<File>,
    parent_stdout: &mut Option<File>,
    parent_stderr: &mut Option<File>,
) -> io::Result<(Option<Arc<File>>, Option<Arc<File>>, Option<Arc<File>>)> {
    fn prepare_pipe(
        parent_writes: bool,
        parent_ref: &mut Option<File>,
        child_ref: &mut Option<Arc<File>>,
    ) -> io::Result<()> {
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
        os::set_inheritable(&file, true)?;
        *child_ref = Some(Arc::new(file));
        Ok(())
    }
    fn prepare_shared_file(file: Arc<File>, child_ref: &mut Option<Arc<File>>) -> io::Result<()> {
        os::set_inheritable(&file, true)?;
        *child_ref = Some(file);
        Ok(())
    }
    fn prepare_null_file(for_read: bool, child_ref: &mut Option<Arc<File>>) -> io::Result<()> {
        let file = if for_read {
            OpenOptions::new().read(true).open(os::NULL_DEVICE)?
        } else {
            OpenOptions::new().write(true).open(os::NULL_DEVICE)?
        };
        prepare_file(file, child_ref)
    }
    fn reuse_stream(
        dest: &mut Option<Arc<File>>,
        src: &mut Option<Arc<File>>,
        src_id: StandardStream,
    ) -> io::Result<()> {
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
        Redirection::Pipe => prepare_pipe(true, parent_stdin, &mut child_stdin)?,
        Redirection::File(file) => prepare_file(file, &mut child_stdin)?,
        Redirection::SharedFile(file) => prepare_shared_file(file, &mut child_stdin)?,
        Redirection::Null => prepare_null_file(true, &mut child_stdin)?,
        Redirection::Merge => {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Redirection::Merge not valid for stdin",
            ));
        }
        Redirection::None => (),
    };
    match stdout {
        Redirection::Pipe => prepare_pipe(false, parent_stdout, &mut child_stdout)?,
        Redirection::File(file) => prepare_file(file, &mut child_stdout)?,
        Redirection::SharedFile(file) => prepare_shared_file(file, &mut child_stdout)?,
        Redirection::Null => prepare_null_file(false, &mut child_stdout)?,
        Redirection::Merge => merge = MergeKind::OutToErr,
        Redirection::None => (),
    };
    match stderr {
        Redirection::Pipe => prepare_pipe(false, parent_stderr, &mut child_stderr)?,
        Redirection::File(file) => prepare_file(file, &mut child_stderr)?,
        Redirection::SharedFile(file) => prepare_shared_file(file, &mut child_stderr)?,
        Redirection::Null => prepare_null_file(false, &mut child_stderr)?,
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

/// Exit status of a process.
///
/// This is an opaque type that wraps the platform's native exit status
/// representation. Use the provided methods to query the exit status.
///
/// On Unix, the raw value is the status from `waitpid()`. On Windows, it
/// is the exit code from `GetExitCodeProcess()`.
#[derive(Eq, PartialEq, Copy, Clone)]
pub struct ExitStatus(pub(crate) Option<os::RawExitStatus>);

impl ExitStatus {
    /// Create an `ExitStatus` from the raw platform value.
    pub(crate) fn from_raw(raw: os::RawExitStatus) -> ExitStatus {
        ExitStatus(Some(raw))
    }

    /// True if the exit status of the process is 0.
    pub fn success(&self) -> bool {
        self.code() == Some(0)
    }

    /// True if the subprocess was killed by a signal with the specified
    /// number.
    ///
    /// Always returns `false` on Windows.
    pub fn is_killed_by(&self, signum: i32) -> bool {
        self.signal() == Some(signum)
    }
}

#[derive(Debug, Copy, Clone)]
#[allow(dead_code)]
pub(crate) enum StandardStream {
    Input = 0,
    Output = 1,
    Error = 2,
}

fn get_standard_stream(which: StandardStream) -> io::Result<Arc<File>> {
    static STREAMS: [OnceLock<Arc<File>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let lock = &STREAMS[which as usize];
    if let Some(stream) = lock.get() {
        return Ok(Arc::clone(stream));
    }
    let stream = os::make_standard_stream(which)?;
    Ok(Arc::clone(lock.get_or_init(|| stream)))
}

#[cfg(unix)]
pub(crate) mod os {
    use super::*;

    pub const NULL_DEVICE: &str = "/dev/null";

    use crate::posix;
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::unix::io::AsRawFd;

    pub use crate::posix::make_standard_stream;

    /// Read exactly N bytes, or return None on immediate EOF. Similar to
    /// read_exact(), but distinguishes between no read and partial read
    /// (which is treated as error).
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn os_start(
        argv: Vec<OsString>,
        child_ends: (Option<Arc<File>>, Option<Arc<File>>, Option<Arc<File>>),
        detached: bool,
        executable: Option<&OsStr>,
        env: Option<&[(OsString, OsString)]>,
        cwd: Option<&OsStr>,
        setuid: Option<u32>,
        setgid: Option<u32>,
        setpgid: bool,
    ) -> io::Result<Process> {
        let mut exec_fail_pipe = posix::pipe()?;
        set_inheritable(&exec_fail_pipe.0, false)?;
        set_inheritable(&exec_fail_pipe.1, false)?;

        let child_env = env.map(format_env);
        let cmd_to_exec = executable.unwrap_or(&argv[0]);
        let just_exec = posix::prep_exec(cmd_to_exec, &argv, child_env.as_deref())?;

        let pid;
        unsafe {
            match posix::fork()? {
                Some(child_pid) => {
                    pid = child_pid;
                }
                None => {
                    drop(exec_fail_pipe.0);
                    let result = do_exec(just_exec, child_ends, cwd, setuid, setgid, setpgid);
                    let error_code = match result {
                        Ok(()) => unreachable!(),
                        Err(e) => e.raw_os_error().unwrap_or(-1),
                    } as u32;
                    exec_fail_pipe.1.write_all(&error_code.to_le_bytes()).ok();
                    posix::_exit(127);
                }
            }
        }

        drop(exec_fail_pipe.1);
        match read_exact_or_eof::<4>(&mut exec_fail_pipe.0)? {
            None => Ok(Process::new(pid, (), detached)),
            Some(error_buf) => {
                let error_code = u32::from_le_bytes(error_buf);
                Err(io::Error::from_raw_os_error(error_code as i32))
            }
        }
    }

    fn format_env(env: &[(OsString, OsString)]) -> Vec<OsString> {
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
            std::env::set_current_dir(cwd)?;
        }

        let (stdin, stdout, stderr) = child_ends;
        dup2_if_needed(stdin, 0)?;
        dup2_if_needed(stdout, 1)?;
        dup2_if_needed(stderr, 2)?;
        posix::reset_sigpipe()?;

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

    pub fn set_inheritable(f: &File, inheritable: bool) -> io::Result<()> {
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

    pub type RawExitStatus = i32;

    impl ExitStatus {
        /// Returns the exit code if the process exited normally.
        ///
        /// On Unix, this returns `Some` only if the process exited
        /// voluntarily (not killed by a signal).
        pub fn code(&self) -> Option<u32> {
            let raw = self.0?;
            libc::WIFEXITED(raw).then(|| libc::WEXITSTATUS(raw) as u32)
        }

        /// Returns the signal number if the process was killed by a signal.
        pub fn signal(&self) -> Option<i32> {
            let raw = self.0?;
            libc::WIFSIGNALED(raw).then(|| libc::WTERMSIG(raw))
        }
    }

    impl fmt::Display for ExitStatus {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.0 {
                Some(raw) if libc::WIFEXITED(raw) => {
                    write!(f, "exit code {}", libc::WEXITSTATUS(raw))
                }
                Some(raw) if libc::WIFSIGNALED(raw) => {
                    write!(f, "signal {}", libc::WTERMSIG(raw))
                }
                Some(raw) => {
                    write!(f, "unrecognized wait status: {} {:#x}", raw, raw)
                }
                None => write!(f, "undetermined exit status"),
            }
        }
    }

    impl fmt::Debug for ExitStatus {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.0 {
                Some(raw) if libc::WIFEXITED(raw) => {
                    write!(f, "ExitStatus(Exited({}))", libc::WEXITSTATUS(raw))
                }
                Some(raw) if libc::WIFSIGNALED(raw) => {
                    write!(f, "ExitStatus(Signal({}))", libc::WTERMSIG(raw))
                }
                Some(raw) => {
                    write!(f, "ExitStatus(Unknown({} {:#x}))", raw, raw)
                }
                None => write!(f, "ExitStatus(Undetermined)"),
            }
        }
    }
}

#[cfg(windows)]
pub(crate) mod os {
    use super::*;

    pub const NULL_DEVICE: &str = "nul";

    use std::collections::HashSet;
    use std::env;
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{AsRawHandle, RawHandle};

    use crate::win32;
    pub use crate::win32::make_standard_stream;

    pub(crate) fn os_start(
        argv: Vec<OsString>,
        child_ends: (Option<Arc<File>>, Option<Arc<File>>, Option<Arc<File>>),
        detached: bool,
        executable: Option<&OsStr>,
        env: Option<&[(OsString, OsString)]>,
        cwd: Option<&OsStr>,
        creation_flags: u32,
    ) -> io::Result<Process> {
        fn raw(opt: Option<&Arc<File>>) -> Option<RawHandle> {
            opt.map(|f| f.as_raw_handle())
        }

        let (mut child_stdin, mut child_stdout, mut child_stderr) = child_ends;
        ensure_child_stream(&mut child_stdin, StandardStream::Input)?;
        ensure_child_stream(&mut child_stdout, StandardStream::Output)?;
        ensure_child_stream(&mut child_stderr, StandardStream::Error)?;
        let cmdline = assemble_cmdline(argv)?;
        let env_block = env.map(|e| format_env_block(e));
        let executable_located = executable.map(|e| locate_in_path(e.to_owned()));
        let (handle, pid) = win32::CreateProcess(
            executable_located.as_ref().map(OsString::as_ref),
            &cmdline,
            env_block.as_deref(),
            cwd,
            true,
            creation_flags,
            raw(child_stdin.as_ref()),
            raw(child_stdout.as_ref()),
            raw(child_stderr.as_ref()),
            win32::STARTF_USESTDHANDLES,
        )?;
        Ok(Process::new(pid as u32, ExtProcessState(handle), detached))
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

    fn ensure_child_stream(
        stream: &mut Option<Arc<File>>,
        which: StandardStream,
    ) -> io::Result<()> {
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
    /// Both handles are created inheritable; callers should use
    /// `set_inheritable` to make the parent's end non-inheritable before
    /// spawning children.
    pub fn make_pipe() -> io::Result<(File, File)> {
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

    pub type RawExitStatus = u32;

    impl ExitStatus {
        /// Returns the exit code if the process exited normally.
        ///
        /// On Windows, this always returns `Some` for a determined exit
        /// status.
        pub fn code(&self) -> Option<u32> {
            self.0
        }

        /// Returns the signal number if the process was killed by a signal.
        ///
        /// On Windows, this always returns `None`.
        pub fn signal(&self) -> Option<i32> {
            None
        }
    }

    impl fmt::Display for ExitStatus {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.0 {
                Some(code) => write!(f, "exit code {}", code),
                None => write!(f, "undetermined exit status"),
            }
        }
    }

    impl fmt::Debug for ExitStatus {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.0 {
                Some(code) => {
                    write!(f, "ExitStatus(Exited({}))", code)
                }
                None => write!(f, "ExitStatus(Undetermined)"),
            }
        }
    }
}
