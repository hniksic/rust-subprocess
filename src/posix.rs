use std::io::{Result, Error};
use std::ffi::{OsStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::fs::File;
use std::os::unix::io::FromRawFd;
use std::ptr;
use std::mem;
use std::iter;
use std::env;

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

pub fn execvp<S1, S2>(cmd: S1, args: &[S2]) -> Result<()>
    where S1: AsRef<OsStr>, S2: AsRef<OsStr>
{
    let cmd_cstring = os_to_cstring(cmd.as_ref())?;
    let argvec = CVec::new(args)?;

    check_err(unsafe {
        libc::execvp(cstring_ptr(&cmd_cstring), argvec.as_c_vec())
    })?;

    unreachable!();
}

pub fn execvpe<S1, S2, S3>(cmd: S1, args: &[S2], env: &[S3]) -> Result<()>
    where S1: AsRef<OsStr>,
          S2: AsRef<OsStr>,
          S3: AsRef<OsStr>
{
    let cmd_osstr = cmd.as_ref();
    let argvec = CVec::new(args)?;
    let envvec = CVec::new(env)?;

    // execvpe is not POSIX, so we must emulate it

    if cmd_osstr.as_bytes().iter().any(|&c| c == b'/') {
        let cmd_cstring = os_to_cstring(cmd_osstr)?;
        check_err(unsafe {
            libc::execve(cstring_ptr(&cmd_cstring),
                         argvec.as_c_vec(), envvec.as_c_vec())
        })?;
        unreachable!();
    } else {
        if let Some(path) = env::var_os("PATH") {
            for pathdir in env::split_paths(&path) {
                let exe_path = pathdir.join(cmd_osstr);
                let exe_cstring = os_to_cstring(exe_path.as_os_str())?;
                unsafe {
                    libc::execve(cstring_ptr(&exe_cstring),
                                 argvec.as_c_vec(), envvec.as_c_vec());
                }
            }
        }
        return Err(Error::from_raw_os_error(libc::ENOENT));
    }
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
