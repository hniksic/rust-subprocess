use std::io::{Result, Error};
use std::path::Path;
use libc;
use std::os::unix::ffi::OsStrExt;
use std::fs::File;
use std::os::unix::io::FromRawFd;
use std::ptr;
use std::ffi::CString;

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum ExitStatus {
    Exited(u8),
    Signaled(u8),
    Other(i32),
}

fn check_err<T: Ord + Default>(num: T) -> Result<T> {
    if num < T::default() {
        return Err(Error::last_os_error());
    }
    Ok(num)
}

fn path_to_cstring(p: &Path) -> (CString, *const libc::c_char) {
    let holder = CString::new(p.as_os_str().as_bytes()).unwrap();
    let ptr;
    {
        let c_bytes = holder.as_bytes_with_nul();
        ptr = &c_bytes[0] as *const u8 as *const libc::c_char;
    }
    (holder, ptr)
}

pub fn pipe() -> Result<(File, File)> {
    let mut fds = [0 as libc::c_int; 2];
    try!(check_err(unsafe { libc::pipe(&mut fds[0]) }));
    Ok(unsafe {
        (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1]))
    })
}

pub fn fork() -> Result<u32> {
    check_err(unsafe { libc::fork() }).map(|pid| pid as u32)
}

use std::fmt::Debug;

pub fn execvp<P1, P2>(cmd: P1, args: &[P2]) -> Result<()>
    where P1: AsRef<Path> + Debug, P2: AsRef<Path> {
    let cstrings: Vec<_> = args.iter()
        .map(|x| path_to_cstring(x.as_ref())).collect();
    let mut args_os: Vec<_> = cstrings.iter().map(|&(_, ptr)| ptr).collect();
    args_os.push(ptr::null());
    let argv = &args_os[0] as *const *const libc::c_char;

    let cmd = path_to_cstring(cmd.as_ref());

    try!(check_err(unsafe { libc::execvp(cmd.1, argv) }));
    Ok(())
}

pub fn _exit(status: u8) -> ! {
    unsafe { libc::_exit(status as libc::c_int) }
}

pub const WNOHANG: i32 = libc::WNOHANG;

pub fn waitpid(pid: u32, flags: i32) -> Result<(u32, ExitStatus)> {
    let mut status = 0 as libc::c_int;
    let pid = try!(check_err(unsafe {
        libc::waitpid(pid as libc::pid_t, &mut status as *mut libc::c_int,
                      flags as libc::c_int)
    }));
    Ok((pid as u32, decode_exit_status(status)))
}

fn decode_exit_status(status: i32) -> ExitStatus {
    unsafe {
        if libc::WIFEXITED(status) {
            ExitStatus::Exited(libc::WEXITSTATUS(status) as u8)
        } else if libc::WIFSIGNALED(status) {
            ExitStatus::Signaled(libc::WTERMSIG(status) as u8)
        } else {
            ExitStatus::Other(status)
        }
    }
}

pub const SIGTERM: u8 = libc::SIGTERM as u8;
pub const SIGKILL: u8 = libc::SIGKILL as u8;

pub fn kill(pid: u32, signal: u8) -> Result<()> {
    try!(check_err(unsafe {
        libc::kill(pid as libc::c_int, signal as libc::c_int)
    }));
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
    try!(check_err(unsafe {
        libc::dup2(oldfd, newfd)
    }));
    Ok(())
}
