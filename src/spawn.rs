use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io;
use std::io::ErrorKind;
use std::sync::{Arc, OnceLock};

use crate::exec::Redirection;
#[cfg(windows)]
use crate::process::ExtProcessState;
use crate::process::Process;

pub(crate) use os::OsOptions;
pub use os::make_pipe;

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
    stdin: Arc<Redirection>,
    stdout: Arc<Redirection>,
    stderr: Arc<Redirection>,
    detached: bool,
    executable: Option<&OsStr>,
    env: Option<&[(OsString, OsString)]>,
    cwd: Option<&OsStr>,
    os_options: OsOptions,
) -> io::Result<SpawnResult> {
    if argv.is_empty() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "argv must not be empty",
        ));
    }

    let (parent_ends, child_ends) = setup_streams(stdin, stdout, stderr)?;

    let process = os::os_start(argv, child_ends, detached, executable, env, cwd, os_options)?;

    Ok(SpawnResult {
        process,
        stdin: parent_ends.0,
        stdout: parent_ends.1,
        stderr: parent_ends.2,
    })
}

fn child_file(r: &Redirection) -> &File {
    match r {
        Redirection::File(f) => f,
        _ => unreachable!(),
    }
}

/// Translate a single `Redirection` into the child-side fd and (for Pipe) the parent-side
/// fd. Returns `(parent_end, child_end)` where only Pipe produces a parent end and None
/// produces neither.
///
/// Merge is not handled here - the caller checks for Merge before calling this function.
fn prepare_child_stream(
    redir: Arc<Redirection>,
    is_input: bool,
) -> io::Result<(Option<File>, Option<Arc<Redirection>>)> {
    // File is the only variant holding a resource - handle specially to avoid dup() when
    // the Arc is shared.
    if matches!(&*redir, Redirection::File(_)) {
        return match Arc::try_unwrap(redir) {
            Ok(Redirection::File(f)) => Ok((None, Some(prepare_file(f)?))),
            Err(arc) => Ok((None, Some(prepare_file_shared(arc)?))),
            _ => unreachable!(),
        };
    }
    // Other variants are trivially cheap - just peek and handle.
    match &*redir {
        Redirection::Pipe => {
            let (parent, child) = prepare_pipe(is_input)?;
            Ok((Some(parent), Some(child)))
        }
        Redirection::Null => Ok((None, Some(prepare_null_file(is_input)?))),
        Redirection::None => Ok((None, None)),
        _ => unreachable!(),
    }
}

fn prepare_pipe(parent_writes: bool) -> io::Result<(File, Arc<Redirection>)> {
    let (read, write) = os::make_pipe()?;
    let (parent_end, child_end) = if parent_writes {
        (write, read)
    } else {
        (read, write)
    };
    os::set_inheritable(&parent_end, false)?;
    Ok((parent_end, Arc::new(Redirection::File(child_end))))
}

fn prepare_file(file: File) -> io::Result<Arc<Redirection>> {
    os::set_inheritable(&file, true)?;
    Ok(Arc::new(Redirection::File(file)))
}

fn prepare_file_shared(arc: Arc<Redirection>) -> io::Result<Arc<Redirection>> {
    os::set_inheritable(child_file(&arc), true)?;
    Ok(arc)
}

fn prepare_null_file(for_read: bool) -> io::Result<Arc<Redirection>> {
    let file = if for_read {
        OpenOptions::new().read(true).open(os::NULL_DEVICE)?
    } else {
        OpenOptions::new().write(true).open(os::NULL_DEVICE)?
    };
    prepare_file(file)
}

// Share a child stream via Arc::clone - zero dup syscalls.
fn reuse_stream(
    dest: &mut Option<Arc<Redirection>>,
    src: &mut Option<Arc<Redirection>>,
    src_id: StandardStream,
) -> io::Result<()> {
    if src.is_none() {
        *src = Some(get_redirection_to_standard_stream(src_id)?);
    }
    *dest = src.clone();
    Ok(())
}

// Set up streams for the child process. Returns (parent_ends, child_ends).
//
// Child ends use Arc<Redirection> (always the File variant internally) so that Merge
// (e.g. 2>&1) can share an fd between stdout and stderr without a dup syscall -
// reuse_stream just does Arc::clone. Dropping an Arc after dup2 in the child only
// decrements the refcount rather than closing the fd, which is important when two
// child_ends reference the same underlying file. For pipeline members that share a
// Redirection::File via Arc, we also avoid dup by reusing the same Arc directly.
fn setup_streams(
    stdin: Arc<Redirection>,
    stdout: Arc<Redirection>,
    stderr: Arc<Redirection>,
) -> io::Result<(
    (Option<File>, Option<File>, Option<File>),
    (
        Option<Arc<Redirection>>,
        Option<Arc<Redirection>>,
        Option<Arc<Redirection>>,
    ),
)> {
    #[derive(PartialEq, Eq, Copy, Clone)]
    enum MergeKind {
        ErrToOut, // 2>&1
        OutToErr, // 1>&2
        None,
    }

    if matches!(&*stdin, Redirection::Merge) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "Redirection::Merge not valid for stdin",
        ));
    }
    let merge = match (
        matches!(&*stdout, Redirection::Merge),
        matches!(&*stderr, Redirection::Merge),
    ) {
        (false, false) => MergeKind::None,
        (false, true) => MergeKind::ErrToOut,
        (true, false) => MergeKind::OutToErr,
        (true, true) => {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "Redirection::Merge not valid for both stdout and stderr",
            ));
        }
    };

    let (parent_stdin, child_stdin) = prepare_child_stream(stdin, true)?;
    let (parent_stdout, mut child_stdout) = if merge == MergeKind::OutToErr {
        (None, None)
    } else {
        prepare_child_stream(stdout, false)?
    };
    let (parent_stderr, mut child_stderr) = if merge == MergeKind::ErrToOut {
        (None, None)
    } else {
        prepare_child_stream(stderr, false)?
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

    Ok((
        (parent_stdin, parent_stdout, parent_stderr),
        (child_stdin, child_stdout, child_stderr),
    ))
}

#[derive(Debug, Copy, Clone)]
#[allow(dead_code)]
pub(crate) enum StandardStream {
    Input = 0,
    Output = 1,
    Error = 2,
}

fn get_redirection_to_standard_stream(which: StandardStream) -> io::Result<Arc<Redirection>> {
    static STREAMS: [OnceLock<Arc<Redirection>>; 3] =
        [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let lock = &STREAMS[which as usize];
    if let Some(stream) = lock.get() {
        return Ok(Arc::clone(stream));
    }
    let stream = os::make_redirection_to_standard_stream(which)?;
    Ok(Arc::clone(lock.get_or_init(|| stream)))
}

#[cfg(unix)]
pub(crate) mod os {
    use super::*;

    #[derive(Clone, Default)]
    pub struct OsOptions {
        pub setuid: Option<u32>,
        pub setgid: Option<u32>,
        pub setpgid: Option<u32>,
    }

    impl OsOptions {
        pub fn setpgid_is_set(&self) -> bool {
            self.setpgid.is_some()
        }
        pub fn set_pgid_value(&mut self, pgid: u32) {
            self.setpgid = Some(pgid);
        }
    }

    pub const NULL_DEVICE: &str = "/dev/null";

    use crate::posix;
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::unix::io::AsRawFd;

    pub use crate::posix::make_redirection_to_standard_stream;

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

    pub(crate) fn os_start(
        argv: Vec<OsString>,
        child_ends: (
            Option<Arc<Redirection>>,
            Option<Arc<Redirection>>,
            Option<Arc<Redirection>>,
        ),
        detached: bool,
        executable: Option<&OsStr>,
        env: Option<&[(OsString, OsString)]>,
        cwd: Option<&OsStr>,
        os_options: OsOptions,
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
                    let result = do_exec(just_exec, child_ends, cwd, &os_options);
                    let error_code = match result {
                        Ok(()) => unreachable!(),
                        Err(e) => e.raw_os_error().unwrap_or(-1),
                    } as u32;
                    exec_fail_pipe.1.write_all(&error_code.to_le_bytes()).ok();
                    posix::_exit(127);
                }
            }
        }

        // Close the parent's copies of child-end fds promptly after fork,
        // before blocking on exec_fail_pipe.
        drop(child_ends);

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

    fn dup2_if_needed(end: Option<Arc<Redirection>>, target_fd: i32) -> io::Result<()> {
        if let Some(r) = &end
            && child_file(r).as_raw_fd() != target_fd
        {
            posix::dup2(child_file(r).as_raw_fd(), target_fd)?;
        }
        Ok(())
    }

    fn do_exec(
        just_exec: impl FnOnce() -> io::Result<()>,
        child_ends: (
            Option<Arc<Redirection>>,
            Option<Arc<Redirection>>,
            Option<Arc<Redirection>>,
        ),
        cwd: Option<&OsStr>,
        os_options: &OsOptions,
    ) -> io::Result<()> {
        if let Some(cwd) = cwd {
            std::env::set_current_dir(cwd)?;
        }

        let (stdin, stdout, stderr) = child_ends;
        dup2_if_needed(stdin, 0)?;
        dup2_if_needed(stdout, 1)?;
        dup2_if_needed(stderr, 2)?;
        posix::reset_sigpipe()?;

        if let Some(gid) = os_options.setgid {
            posix::setgid(gid)?;
        }
        if let Some(uid) = os_options.setuid {
            posix::setuid(uid)?;
        }
        if let Some(pgid) = os_options.setpgid {
            posix::setpgid(0, pgid)?;
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
}

#[cfg(windows)]
pub(crate) mod os {
    use super::*;

    #[derive(Clone, Default)]
    pub struct OsOptions {
        pub creation_flags: u32,
    }

    pub const NULL_DEVICE: &str = "nul";

    use std::collections::HashSet;
    use std::env;
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{AsRawHandle, RawHandle};

    use crate::win32;
    pub use crate::win32::make_redirection_to_standard_stream;

    pub(crate) fn os_start(
        argv: Vec<OsString>,
        child_ends: (
            Option<Arc<Redirection>>,
            Option<Arc<Redirection>>,
            Option<Arc<Redirection>>,
        ),
        detached: bool,
        executable: Option<&OsStr>,
        env: Option<&[(OsString, OsString)]>,
        cwd: Option<&OsStr>,
        os_options: OsOptions,
    ) -> io::Result<Process> {
        fn raw(opt: Option<&Arc<Redirection>>) -> Option<RawHandle> {
            opt.map(|r| child_file(r).as_raw_handle())
        }

        let (mut child_stdin, mut child_stdout, mut child_stderr) = child_ends;
        ensure_child_stream(&mut child_stdin, StandardStream::Input)?;
        ensure_child_stream(&mut child_stdout, StandardStream::Output)?;
        ensure_child_stream(&mut child_stderr, StandardStream::Error)?;
        let cmdline = assemble_cmdline(argv)?;
        let env_block = env.map(format_env_block);
        let executable_located = executable.map(|e| locate_in_path(e.to_owned()));
        let (handle, pid) = win32::CreateProcess(
            executable_located.as_ref().map(OsString::as_ref),
            &cmdline,
            env_block.as_deref(),
            cwd,
            true,
            os_options.creation_flags,
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
        stream: &mut Option<Arc<Redirection>>,
        which: StandardStream,
    ) -> io::Result<()> {
        if stream.is_none() {
            *stream = Some(get_redirection_to_standard_stream(which)?);
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
    /// Both handles are created inheritable; callers should use `set_inheritable` to make
    /// the parent's end non-inheritable before spawning children.
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

    // Translated from ArgvQuote at
    // https://learn.microsoft.com/en-us/archive/blogs/twistylittlepassagesallalike/everyone-quotes-command-line-arguments-the-wrong-way
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
}
