use std::io::{Result, Error};
use std::ffi::{OsStr, OsString, CString};
use std::os::unix::ffi::OsStrExt;
use std::fs::File;
use std::os::unix::io::FromRawFd;
use std::ptr;
use std::mem;
use std::iter;
use std::env;
use std::cell::RefCell;

use libc;

use os_common::{ExitStatus, StandardStream, Undropped};

pub use libc::ECHILD;

fn check_err<T: Ord + Default>(num: T) -> Result<T> {
    if num < T::default() {
        return Err(Error::last_os_error());
    }
    Ok(num)
}

pub fn pipe() -> Result<(File, File)> {
    let mut fds = [0 as libc::c_int; 2];
    check_err(unsafe { libc::pipe(fds.as_mut_ptr()) })?;
    Ok(unsafe {
        (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1]))
    })
}

pub fn fork() -> Result<u32> {
    check_err(unsafe { libc::fork() }).map(|pid| pid as u32)
}

fn os_to_cstring(s: &OsStr) -> Result<CString> {
    let bytes = s.as_bytes();
    if bytes.iter().any(|&b| b == 0) {
        return Err(Error::from_raw_os_error(libc::EINVAL));
    }
    Ok(CString::new(bytes)
       // not expected to fail on Unix, as Unix paths *are* C strings
       .expect("converting Unix path to C string"))
}

fn cstring_ptr(s: &CString) -> *const libc::c_char {
    s.as_bytes_with_nul().as_ptr() as _
}

#[derive(Debug)]
struct CVec {
    // Individual C strings; they are not unused as rustc thinks, they
    // are pointed to by elements of self.ptrs.
    #[allow(dead_code)]
    strings: Vec<CString>,

    // nullptr-terminated vector of pointers to data inside
    // self.strings.
    ptrs: Vec<*const libc::c_char>,
}

impl CVec {
    fn new<S>(slice: &[S]) -> Result<CVec>
        where S: AsRef<OsStr>
    {
        let maybe_vec_cstring: Result<Vec<CString>> = slice.iter()
            .map(|x| os_to_cstring(x.as_ref())).collect();
        let vec_cstring = maybe_vec_cstring?;
        let ptrs: Vec<_> = vec_cstring.iter().map(cstring_ptr)
            .chain(iter::once(ptr::null())).collect();
        Ok(CVec { strings: vec_cstring, ptrs: ptrs })
    }

    pub fn as_c_vec(&self) -> *const *const libc::c_char {
        self.ptrs.as_ptr()
    }
}

struct SplitPath<'a> {
    path: &'a OsStr,
    last: usize,
    current: usize,
}

impl<'a> Iterator for SplitPath<'a> {
    type Item = &'a OsStr;

    fn next(&mut self) -> Option<&'a OsStr> {
        let bytes = self.path.as_bytes();
        for i in self.current..bytes.len() {
            if bytes[i] == b':' {
                let piece = OsStr::from_bytes(&bytes[self.last..i]);
                self.last = i + 1;
                if piece.len() != 0 {
                    self.current = i + 1;
                    return Some(piece);
                }
            }
        }
        self.current = bytes.len();
        if self.last != bytes.len() {
            let piece = OsStr::from_bytes(&bytes[self.last..]);
            self.last = bytes.len();
            return Some(piece);
        }
        return None;
    }
}

fn split_path(path: &OsStr) -> SplitPath {
    // Can't use env::split_path because it allocates OsString
    // objects, and we need to iterate over PATH after fork() when
    // allocations are strictly verboten.
    SplitPath {
        path: path,
        last: 0,
        current: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::split_path;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std;

    fn s(s: &str) -> Vec<&str> {
        split_path(OsStr::new(s))
            .map(|osstr| std::str::from_utf8(osstr.as_bytes()).unwrap())
            .collect()
    }

    #[test]
    fn test_split_path() {
        let empty = Vec::<&OsStr>::new();

        assert_eq!(s("a:b"), vec!["a", "b"]);
        assert_eq!(s("one:twothree"), vec!["one", "twothree"]);
        assert_eq!(s("a:"), vec!["a"]);
        assert_eq!(s(""), empty);
        assert_eq!(s(":"), empty);
        assert_eq!(s("::"), empty);
        assert_eq!(s(":::"), empty);
        assert_eq!(s("a::b"), vec!["a", "b"]);
        assert_eq!(s(":a::::b:"), vec!["a", "b"]);
    }
}

struct FinishExec {
    cmd: OsString,
    argvec: CVec,
    envvec: Option<CVec>,
    search_path: Option<OsString>,
    exe_buf: RefCell<Vec<u8>>,
}

impl FinishExec {
    fn new(cmd: OsString, argvec: CVec, envvec: Option<CVec>,
           search_path: Option<OsString>)
           -> FinishExec {
        // Avoid allocation after fork() by pre-allocating the buffer
        // that will be used for constructing the executable C string.

        // Allocate enough room for "<pathdir>/<command>\0", pathdir
        // being the longest component of PATH.
        let mut max_exe_len = cmd.len() + 1;
        if let Some(ref search_path) = search_path {
            // make sure enough room is present for the largest of the
            // PATH components, plus 1 for the intervening '/'.
            max_exe_len += 1 + split_path(search_path)
                .map(|dir| dir.len())
                .max().unwrap_or(0);
        }

        FinishExec {
            cmd: cmd,
            argvec: argvec,
            envvec: envvec,
            search_path: search_path,
            exe_buf: RefCell::new(Vec::with_capacity(max_exe_len)),
        }
    }

    fn finish(&mut self) -> Result<()> {
        // Invoked after fork() - no heap allocation allowed

        if let Some(ref search_path) = self.search_path {
            // POSIX specifies execvp and execve, but not execvpe
            // (although glibc has one), so we have to iterate over
            // PATH ourselves
            for dir in split_path(search_path.as_os_str()) {
                self.set_exe(&[dir.as_bytes(), "/".as_bytes(),
                               self.cmd.as_bytes()]);
                self.exec().ok();
            }
            return Err(Error::last_os_error())
        }

        self.set_exe(&[self.cmd.as_bytes()]);
        self.exec()?;

        // failed exec can only return Err(..)
        unreachable!();
    }

    fn set_exe(&self, byte_slices: &[&[u8]]) {
        let mut exe_buf = self.exe_buf.borrow_mut();
        exe_buf.truncate(0);
        for byte_slice in byte_slices {
            exe_buf.extend_from_slice(byte_slice);
        }
        exe_buf.push(0);
    }

    fn exec(&self) -> Result<()> {
        let exe_buf = self.exe_buf.borrow();

        unsafe {
            match self.envvec.as_ref() {
                Some(ref envvec) =>
                    libc::execve(exe_buf.as_ptr() as *const i8,
                                 self.argvec.as_c_vec(), envvec.as_c_vec()),
                None =>
                    libc::execv(exe_buf.as_ptr() as *const i8,
                                self.argvec.as_c_vec()),
            }
        };
        Err(Error::last_os_error())
    }
}

pub fn stage_exec<S1, S2, S3>(cmd: S1, args: &[S2], env: Option<&[S3]>)
                             -> Result<Box<FnMut() -> Result<()>>>
    where S1: AsRef<OsStr>, S2: AsRef<OsStr>, S3: AsRef<OsStr>
{
    let cmd = cmd.as_ref().to_owned();
    let argvec = CVec::new(args)?;
    let envvec = if let Some(env) = env { Some(CVec::new(env)?) } else { None };

    let search_path = if !cmd.as_bytes().iter().any(|&b| b == b'/') {
        env::var_os("PATH")
            // treat empty path as non-existent
            .and_then(|p| if p.len() == 0 { None } else { Some(p) })
    } else {
        None
    };

    let mut exec = FinishExec::new(cmd, argvec, envvec, search_path);
    Ok(Box::new(move || exec.finish()))
}

pub fn _exit(status: u8) -> ! {
    unsafe { libc::_exit(status as libc::c_int) }
}

pub const WNOHANG: i32 = libc::WNOHANG;

pub fn waitpid(pid: u32, flags: i32) -> Result<(u32, ExitStatus)> {
    let mut status = 0 as libc::c_int;
    let pid = check_err(unsafe {
        libc::waitpid(pid as libc::pid_t, &mut status as *mut libc::c_int,
                      flags as libc::c_int)
    })?;
    Ok((pid as u32, decode_exit_status(status)))
}

fn decode_exit_status(status: i32) -> ExitStatus {
    unsafe {
        if libc::WIFEXITED(status) {
            ExitStatus::Exited(libc::WEXITSTATUS(status) as u32)
        } else if libc::WIFSIGNALED(status) {
            ExitStatus::Signaled(libc::WTERMSIG(status) as u8)
        } else {
            ExitStatus::Other(status)
        }
    }
}

pub use libc::{SIGTERM, SIGKILL};

pub fn kill(pid: u32, signal: i32) -> Result<()> {
    check_err(unsafe {
        libc::kill(pid as libc::c_int, signal)
    })?;
    Ok(())
}

pub const F_GETFD: i32 = libc::F_GETFD;
pub const F_SETFD: i32 = libc::F_SETFD;
pub const FD_CLOEXEC: i32 = libc::FD_CLOEXEC;

pub fn fcntl(fd: i32, cmd: i32, arg1: Option<i32>) -> Result<i32> {
    check_err(unsafe {
        match arg1 {
            Some(arg1) => libc::fcntl(fd, cmd, arg1),
            None => libc::fcntl(fd, cmd),
        }
    })
}

pub fn dup2(oldfd: i32, newfd: i32) -> Result<()> {
    check_err(unsafe {
        libc::dup2(oldfd, newfd)
    })?;
    Ok(())
}

pub fn get_standard_stream(which: StandardStream) -> Result<Undropped<File>> {
    let fd = match which {
        StandardStream::Input => 0,
        StandardStream::Output => 1,
        StandardStream::Error => 2,
    };
    unsafe {
        Ok(Undropped::new(File::from_raw_fd(fd)))
    }
}

pub fn reset_sigpipe() -> Result<()> {
    // This is called after forking to reset SIGPIPE handling to the
    // defaults that Unix programs expect.  Quoting
    // std::process::Command::do_exec:
    //
    // """
    // libstd ignores SIGPIPE, and signal-handling libraries often set
    // a mask. Child processes inherit ignored signals and the signal
    // mask from their parent, but most UNIX programs do not reset
    // these things on their own, so we need to clean things up now to
    // avoid confusing the program we're about to run.
    // """

    unsafe {
        let mut set: libc::sigset_t = mem::uninitialized();
        check_err(libc::sigemptyset(&mut set))?;
        check_err(libc::pthread_sigmask(libc::SIG_SETMASK, &set, ptr::null_mut()))?;
        let ret = libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        if ret == libc::SIG_ERR {
            return Err(Error::last_os_error());
        }
    }
    Ok(())
}

#[repr(C)]
pub struct PollFd(libc::pollfd);

impl PollFd {
    pub fn new(fd: Option<i32>, events: i16) -> PollFd {
        PollFd(libc::pollfd {
            fd: fd.unwrap_or(-1),
            events: events,
            revents: 0,
        })
    }
    pub fn test(&self, mask: i16) -> bool {
        return self.0.revents & mask != 0
    }
}

pub use libc::{
    POLLIN,
    POLLOUT,
    POLLERR,
    POLLHUP,
    POLLPRI,
    POLLNVAL,
};

pub fn poll(fds: &mut [PollFd], timeout: Option<u32>) -> Result<usize> {
    let cnt;
    let timeout = timeout
        .map(|t|
             if t > i32::max_value() as u32 { i32::max_value() }
             else { t as i32 })
        .unwrap_or(-1);
    unsafe {
        let fds_ptr = fds.as_ptr() as *mut libc::pollfd;
        cnt = check_err(libc::poll(fds_ptr, fds.len() as libc::nfds_t,
                                   timeout))?;
    }
    Ok(cnt as usize)
}
