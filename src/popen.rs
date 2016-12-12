use std::path::{PathBuf, Path};
use std::io::{Result, Error, Read, Write};
use std::mem;
use std::fs::File;

use posix;
pub use posix::{SIGKILL, SIGTERM, ExitStatus};

#[derive(Debug)]
pub struct Popen {
    pid: Option<u32>,
    exit_status: Option<ExitStatus>,
}


fn set_cloexec(f: &File) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    let fd = f.as_raw_fd();
    let old = try!(posix::fcntl(fd, posix::F_GETFD, None));
    try!(posix::fcntl(fd, posix::F_SETFD, Some(old | posix::FD_CLOEXEC)));
    Ok(())
}


impl Popen {
    pub fn create<P: AsRef<Path>>(args: &[P]) -> Result<Popen> {
        let args: Vec<PathBuf> = args.iter()
            .map(|p| p.as_ref().to_owned()).collect();
        let mut inst = Popen {
            pid: None,
            exit_status: None,
        };
        try!(inst.start(args));
        Ok(inst)
    }

    fn start(&mut self, args: Vec<PathBuf>) -> Result<()> {
        let mut exec_fail_pipe = try!(posix::pipe());
        try!(set_cloexec(&exec_fail_pipe.0));
        try!(set_cloexec(&exec_fail_pipe.1));
        let child_pid = try!(posix::fork());
        if child_pid == 0 {
            mem::drop(exec_fail_pipe.0);
            let result = posix::execvp(&args[0], &args);
            // Notify the parent process that exec has failed, and exit.
            let error_code: i32 = match result {
                Ok(()) => unreachable!(),
                Err(e) => e.raw_os_error().unwrap_or(-1)
            };
            // XXX use the byteorder crate to serialize the error
            exec_fail_pipe.1.write_all(format!("{}", error_code).as_bytes()).unwrap();
            posix::_exit(127);
        }
        self.pid = Some(child_pid as u32);
        mem::drop(exec_fail_pipe.1);
        let mut error_string = String::new();
        try!(exec_fail_pipe.0.read_to_string(&mut error_string));
        if error_string.len() != 0 {
            let error_code: i32 = error_string.parse().unwrap();
            Err(Error::from_raw_os_error(error_code))
        } else {
            Ok(())
        }
    }

    fn wait_with(&mut self, wait_flags: i32) -> Result<Option<ExitStatus>> {
        match self.pid {
            Some(pid) => {
                // XXX handle some kinds of error - at least ECHILD and EINTR
                let (pid_out, exit_status) = try!(posix::waitpid(pid, wait_flags));
                if pid_out == pid {
                    self.pid = None;
                    self.exit_status = Some(exit_status);
                }
            },
            None => (),
        }
        Ok(self.exit_status)
    }

    pub fn wait(&mut self) -> Result<Option<ExitStatus>> {
        self.wait_with(0)
    }

    pub fn poll(&mut self) -> Option<ExitStatus> {
        self.wait_with(posix::WNOHANG).unwrap_or(None)
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
