use std::path::{PathBuf, Path};
use std::io::{Result, Error, Read, Write};
use std::mem;
use std::fs::File;
use std::os::unix::io::AsRawFd;

use posix;
pub use posix::{SIGKILL, SIGTERM, ExitStatus};

#[derive(Debug)]
pub struct Popen {
    pid: Option<u32>,
    exit_status: Option<ExitStatus>,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
}


fn set_cloexec(f: &File) -> Result<()> {
    let fd = f.as_raw_fd();
    let old = try!(posix::fcntl(fd, posix::F_GETFD, None));
    try!(posix::fcntl(fd, posix::F_SETFD, Some(old | posix::FD_CLOEXEC)));
    Ok(())
}

#[derive(Debug)]
pub enum Redirection {
    None,
    //File(File),
    Pipe,
}

impl Popen {
    pub fn create_full<P: AsRef<Path>>(
        args: &[P], stdin: Redirection, stdout: Redirection, stderr: Redirection)
        -> Result<Popen>
    {
        let args: Vec<PathBuf> = args.iter()
            .map(|p| p.as_ref().to_owned()).collect();
        let mut inst = Popen {
            pid: None,
            exit_status: None,
            stdin: None,
            stdout: None,
            stderr: None,
        };
        try!(inst.start(args, stdin, stdout, stderr));
        Ok(inst)
    }

    pub fn create<P: AsRef<Path>>(args: &[P]) -> Result<Popen> {
        Popen::create_full(args, Redirection::None, Redirection::None, Redirection::None)
    }

    fn start(&mut self, args: Vec<PathBuf>,
             stdin: Redirection, stdout: Redirection, stderr: Redirection)
             -> Result<()> {
        let mut exec_fail_pipe = try!(posix::pipe());
        try!(set_cloexec(&exec_fail_pipe.0));
        try!(set_cloexec(&exec_fail_pipe.1));
        {
            let child_ends = try!(self.setup_pipes(stdin, stdout, stderr));
            let child_pid = try!(posix::fork());
            if child_pid == 0 {
                mem::drop(exec_fail_pipe.0);
                let result = Popen::do_exec(args, child_ends);
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
        }
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

    fn setup_pipes(&mut self, stdin: Redirection, stdout: Redirection, stderr: Redirection)
                   -> Result<(Option<File>, Option<File>, Option<File>)> {
        let child_stdin = match stdin {
            Redirection::Pipe => {
                let (read, write) = try!(posix::pipe());
                try!(set_cloexec(&write));
                self.stdin = Some(write);
                Some(read)
            }
            Redirection::None => None
        };
        let child_stdout = match stdout {
            Redirection::Pipe => {
                let (read, write) = try!(posix::pipe());
                try!(set_cloexec(&read));
                self.stdout = Some(read);
                Some(write)
            }
            Redirection::None => None
        };
        let child_stderr = match stderr {
            Redirection::Pipe => {
                let (read, write) = try!(posix::pipe());
                try!(set_cloexec(&read));
                self.stderr = Some(read);
                Some(write)
            }
            Redirection::None => None
        };
        Ok((child_stdin, child_stdout, child_stderr))
    }

    fn do_exec(args: Vec<PathBuf>,
               child_ends: (Option<File>, Option<File>, Option<File>)) -> Result<()> {
        let (stdin, stdout, stderr) = child_ends;
        if let Some(stdin) = stdin {
            try!(posix::dup2(stdin.as_raw_fd(), 0));
        }
        if let Some(stdout) = stdout {
            try!(posix::dup2(stdout.as_raw_fd(), 1));
        }
        if let Some(stderr) = stderr {
            try!(posix::dup2(stderr.as_raw_fd(), 2));
        }
        posix::execvp(&args[0], &args)
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
