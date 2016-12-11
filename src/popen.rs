use std::path::{PathBuf, Path};
use std::io::{Result, Error, Read, Write};
use std::mem;

use std::os::unix::io::AsRawFd;

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum ExitStatus {
    Unknown,
    Exited(u8),
    Signaled(u8),
}

#[derive(Debug)]
pub struct Popen {
    args: Vec<PathBuf>,
    pid: Option<u32>,
    exit_status: ExitStatus,
}

pub use self::posix::{SIGKILL, SIGTERM};

mod posix {
    use std::io::{Result, Error};
    use std::path::Path;
    use libc;
    use std::os::unix::ffi::OsStrExt;
    use std::fs::File;
    use std::os::unix::io::FromRawFd;
    use std::ptr;
    use super::ExitStatus;

    fn check_err<T: Ord + Default>(num: T) -> Result<T> {
        if num < T::default() {
            return Err(Error::last_os_error());
        }
        Ok(num)
    }

    fn path_as_ptr(p: &Path) -> *const libc::c_char {
        let c_bytes = p.as_os_str().as_bytes();
        &c_bytes[0] as *const u8 as *const libc::c_char
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

    pub fn execvp<P1, P2>(cmd: P1, args: &[P2]) -> Result<()>
        where P1: AsRef<Path>, P2: AsRef<Path> {
        let mut args_os: Vec<_> = args.iter()
            .map(|x| path_as_ptr(x.as_ref())).collect();
        args_os.push(ptr::null());
        let argv = &args_os[0] as *const *const libc::c_char;
        check_err(unsafe { libc::execvp(path_as_ptr(cmd.as_ref()), argv) })
            .and(Ok(()))
    }

    pub fn _exit(status: u8) -> ! {
        unsafe { libc::_exit(status as libc::c_int) }
    }

    pub const WNOHANG: i32 = libc::WNOHANG;

    pub fn waitpid(pid: u32, flags: i32) -> Result<(u32, ExitStatus)> {
        let mut status = 0 as libc::c_int;
        let pid = try!(check_err(unsafe {
            println!("waiting for {}", pid as libc::pid_t);
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
                ExitStatus::Unknown
            }
        }
    }

    pub const SIGTERM: u8 = libc::SIGTERM as u8;
    pub const SIGKILL: u8 = libc::SIGKILL as u8;

    pub fn kill(pid: u32, signal: u8) -> Result<()> {
        check_err(unsafe {
            println!("killing {} with {}", pid as libc::c_int, signal as libc::c_int);
            libc::kill(pid as libc::c_int, signal as libc::c_int)
        }).and(Ok(()))
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
}

fn set_cloexec(fd: i32) -> Result<()> {
    let old = try!(posix::fcntl(fd, posix::F_GETFD, None));
    posix::fcntl(fd, posix::F_SETFD, Some(old | posix::FD_CLOEXEC))
        .and(Ok(()))
}


impl Popen {
    pub fn create<P: AsRef<Path>>(args: &[P]) -> Result<Popen> {
        let args: Vec<PathBuf> = args.iter()
            .map(|p| p.as_ref().to_owned()).collect();
        let mut inst = Popen {
            args: args,
            pid: None,
            exit_status: ExitStatus::Unknown,
        };
        try!(inst.start());
        Ok(inst)
    }

    fn start(&mut self) -> Result<()> {
        let mut exec_fail_pipe = try!(posix::pipe());
        set_cloexec(exec_fail_pipe.0.as_raw_fd()).unwrap();
        set_cloexec(exec_fail_pipe.1.as_raw_fd()).unwrap();
        let child_pid = try!(posix::fork());
        if child_pid == 0 {
            mem::drop(exec_fail_pipe.0);
            let result = posix::execvp(&self.args[0], &self.args);
            let error_code: i32 = match result {
                Ok(()) => unreachable!(),
                Err(e) => e.raw_os_error().unwrap_or(-1)
            };
            // XXX we don't really need formatting here - we could use
            // the byteorder crate to communicate the i32 over the
            // pipe
            exec_fail_pipe.1.write_all(format!("{}", error_code).as_bytes()).unwrap();
            posix::_exit(127);
        }
        self.pid = Some(child_pid as u32);
        mem::drop(exec_fail_pipe.1);
        let mut error_string = String::new();
        exec_fail_pipe.0.read_to_string(&mut error_string).unwrap();
        if error_string.len() != 0 {
            let error_code: i32 = error_string.parse().unwrap();
            Err(Error::from_raw_os_error(error_code))
        } else {
            Ok(())
        }
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        match self.pid {
            Some(pid) => {
                // XXX handle some kinds of error - at least ECHILD and EINTR
                let (pid_out, exit_status) = try!(posix::waitpid(pid, 0));
                if pid_out == pid {
                    self.pid = None;
                    self.exit_status = exit_status;
                }
            },
            None => (),
        }
        Ok(self.exit_status)
    }

    fn send_signal(&self, signal: u8) -> Result<()> {
        match self.pid {
            Some(pid) => {
                posix::kill(pid, signal)
            },
            None => Ok(()),
        }
    }

    pub fn terminate(&self) -> Result<()> {
        self.send_signal(SIGTERM)
    }

    pub fn kill(&self) -> Result<()> {
        self.send_signal(SIGKILL)
    }
}

impl Drop for Popen {
    fn drop(&mut self) {
        // attempt to reap the child process to avoid leaving a zombie
        match self.pid {
            Some(pid) => { posix::waitpid(pid, posix::WNOHANG).ok(); },
            None => ()
        }
    }
}
