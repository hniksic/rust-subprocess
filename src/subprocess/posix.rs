use std::io::{Result, Error};
use std::path::Path;
use libc;
use std::os::unix::ffi::OsStrExt;
use std::fs::File;
use std::os::unix::io::FromRawFd;
use std::ptr;
use std::ffi::CString;

use subprocess::common::{ExitStatus, StandardStream};

fn check_err<T: Ord + Default>(num: T) -> Result<T> {
    if num < T::default() {
        return Err(Error::last_os_error());
    }
    Ok(num)
}

pub fn pipe() -> Result<(File, File)> {
    let mut fds = [0 as libc::c_int; 2];
    check_err(unsafe { libc::pipe(&mut fds[0]) })?;
    Ok(unsafe {
        (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1]))
    })
}

pub fn fork() -> Result<u32> {
    check_err(unsafe { libc::fork() }).map(|pid| pid as u32)
}

fn path_to_cstring(p: &Path) -> Result<CString> {
    let bytes = p.as_os_str().as_bytes();
    if bytes.iter().any(|&b| b == 0) {
        return Err(Error::from_raw_os_error(libc::EINVAL));
    }
    Ok(CString::new(bytes)
       // not expected to fail on Unix, as Unix paths *are* C strings
       .expect("converting Unix path to C string"))
}

fn cstring_ptr(s: &CString) -> *const libc::c_char {
    &s.as_bytes_with_nul()[0] as *const u8 as _
}

pub fn execvp<P1, P2>(cmd: P1, args: &[P2]) -> Result<()>
    where P1: AsRef<Path>, P2: AsRef<Path> {
    let try_args_cstring: Result<Vec<CString>> = args.iter()
        .map(|x| path_to_cstring(x.as_ref())).collect();
    let args_cstring: Vec<CString> = try_args_cstring?;
    let mut args_ptr: Vec<*const libc::c_char> = args_cstring.iter()
        .map(cstring_ptr).collect();
    args_ptr.push(ptr::null());
    let c_argv = &args_ptr[0] as *const *const libc::c_char;
    let cmd_cstring = path_to_cstring(cmd.as_ref())?;

    check_err(unsafe { libc::execvp(cstring_ptr(&cmd_cstring), c_argv) })?;
    Ok(())
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

pub const SIGTERM: u8 = libc::SIGTERM as u8;
pub const SIGKILL: u8 = libc::SIGKILL as u8;

pub fn kill(pid: u32, signal: u8) -> Result<()> {
    check_err(unsafe {
        libc::kill(pid as libc::c_int, signal as libc::c_int)
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

pub fn get_standard_stream(which: StandardStream) -> File {
    let fd = match which {
        StandardStream::Input => 0,
        StandardStream::Output => 1,
        StandardStream::Error => 2,
    };
    unsafe { File::from_raw_fd(fd) }
}
