use std::path::{PathBuf, Path};
use std::io::{Result, Error, Read, Write};
use std::mem;

use std::os::unix::io::AsRawFd;

use posix;
pub use posix::{SIGKILL, SIGTERM, ExitStatus};

#[derive(Debug)]
pub struct Popen {
    args: Vec<PathBuf>,
    pid: Option<u32>,
    exit_status: ExitStatus,
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
