extern crate crossbeam;

use std::error::Error;
use std::path::{PathBuf, Path};
use std::io;
use std::io::{Read, Write};
use std::mem;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::string::FromUtf8Error;
use std::fmt;

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


fn set_cloexec(f: &File) -> io::Result<()> {
    let fd = f.as_raw_fd();
    let old = try!(posix::fcntl(fd, posix::F_GETFD, None));
    try!(posix::fcntl(fd, posix::F_SETFD, Some(old | posix::FD_CLOEXEC)));
    Ok(())
}

#[derive(Debug)]
pub enum Redirection {
    None,
    File(File),
    Pipe,
}

impl Popen {
    pub fn create_full<P: AsRef<Path>>(
        args: &[P], stdin: Redirection, stdout: Redirection, stderr: Redirection)
        -> io::Result<Popen>
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

    pub fn create<P: AsRef<Path>>(args: &[P]) -> io::Result<Popen> {
        Popen::create_full(args, Redirection::None, Redirection::None, Redirection::None)
    }

    fn start(&mut self, args: Vec<PathBuf>,
             stdin: Redirection, stdout: Redirection, stderr: Redirection)
             -> io::Result<()> {
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
                exec_fail_pipe.1.write_all(format!("{}", error_code).as_bytes())
                    .expect("write to error pipe");
                posix::_exit(127);
            }
            self.pid = Some(child_pid as u32);
        }
        mem::drop(exec_fail_pipe.1);
        let mut error_string = String::new();
        try!(exec_fail_pipe.0.read_to_string(&mut error_string));
        if error_string.len() != 0 {
            let error_code: i32 = error_string.parse()
                .expect("parse child error code");
            Err(io::Error::from_raw_os_error(error_code))
        } else {
            Ok(())
        }
    }

    fn setup_pipes(&mut self, stdin: Redirection, stdout: Redirection, stderr: Redirection)
                   -> io::Result<(Option<File>, Option<File>, Option<File>)> {
        let child_stdin = match stdin {
            Redirection::Pipe => {
                let (read, write) = try!(posix::pipe());
                try!(set_cloexec(&write));
                self.stdin = Some(write);
                Some(read)
            }
            Redirection::File(file) => Some(file),
            Redirection::None => None,
        };
        let child_stdout = match stdout {
            Redirection::Pipe => {
                let (read, write) = try!(posix::pipe());
                try!(set_cloexec(&read));
                self.stdout = Some(read);
                Some(write)
            }
            Redirection::File(file) => Some(file),
            Redirection::None => None
        };
        let child_stderr = match stderr {
            Redirection::Pipe => {
                let (read, write) = try!(posix::pipe());
                try!(set_cloexec(&read));
                self.stderr = Some(read);
                Some(write)
            }
            Redirection::File(file) => Some(file),
            Redirection::None => None
        };
        Ok((child_stdin, child_stdout, child_stderr))
    }

    fn do_exec(args: Vec<PathBuf>,
               child_ends: (Option<File>, Option<File>, Option<File>)) -> io::Result<()> {
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

    fn wait_with(&mut self, wait_flags: i32) -> io::Result<Option<ExitStatus>> {
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

    pub fn wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.wait_with(0)
    }

    pub fn poll(&mut self) -> Option<ExitStatus> {
        self.wait_with(posix::WNOHANG).unwrap_or(None)
    }

    fn send_signal(&self, signal: u8) -> io::Result<()> {
        match self.pid {
            Some(pid) => {
                posix::kill(pid, signal)
            },
            None => Ok(()),
        }
    }

    pub fn terminate(&self) -> io::Result<()> {
        self.send_signal(SIGTERM)
    }

    pub fn kill(&self) -> io::Result<()> {
        self.send_signal(SIGKILL)
    }

    fn read_chunk(f: &mut File, append_to: &mut Vec<u8>) -> io::Result<bool> {
        let mut buf = [0u8; 8192];
        let cnt = try!(f.read(&mut buf));
        if cnt != 0 {
            append_to.extend_from_slice(&buf[..cnt]);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn comm_read(outfile: &mut Option<File>) -> io::Result<Vec<u8>> {
        let mut contents = Vec::new();
        {
            let outfile = outfile.as_mut().expect("file missing");
            while try!(Popen::read_chunk(outfile, &mut contents)) {
            }
        }
        outfile.take();
        Ok(contents)
    }

    fn comm_write(infile: &mut Option<File>, input_data: &[u8]) -> io::Result<()> {
        {
            let infile = infile.as_mut().expect("file missing");
            try!(infile.write_all(input_data));
        }
        infile.take();
        Ok(())
    }

    pub fn communicate_bytes(&mut self, input_data: Option<&[u8]>)
                             -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        match (&mut self.stdin, &mut self.stdout, &mut self.stderr) {
            (mut stdin_ref @ &mut Some(_), &mut None, &mut None) => {
                let input_data = input_data.expect("must provide input to redirected stdin");
                try!(Popen::comm_write(stdin_ref, input_data));
                Ok((None, None))
            }
            (&mut None, mut stdout_ref @ &mut Some(_), &mut None) => {
                assert!(input_data.is_none(), "cannot provide input to non-redirected stdin");
                let out = try!(Popen::comm_read(stdout_ref));
                Ok((Some(out), None))
            }
            (&mut None, &mut None, mut stderr_ref @ &mut Some(_)) => {
                assert!(input_data.is_none(), "cannot provide input to non-redirected stdin");
                let err = try!(Popen::comm_read(stderr_ref));
                Ok((Some(err), None))
            }
            (ref mut stdin_ref, ref mut stdout_ref, ref mut stderr_ref) =>
                crossbeam::scope(move |scope| {
                    let (mut out_thr, mut err_thr) = (None, None);
                    if stdout_ref.is_some() {
                        out_thr = Some(scope.spawn(move || Popen::comm_read(stdout_ref)))
                    }
                    if stderr_ref.is_some() {
                        err_thr = Some(scope.spawn(move || Popen::comm_read(stderr_ref)))
                    }
                    if stdin_ref.is_some() {
                        let input_data = input_data.expect("must provide input to redirected stdin");
                        try!(Popen::comm_write(stdin_ref, input_data));
                    }
                    Ok((if let Some(out_thr) = out_thr {Some(try!(out_thr.join()))} else {None},
                        if let Some(err_thr) = err_thr {Some(try!(err_thr.join()))} else {None}))
                })
        }
    }

    pub fn communicate(&mut self, input_data: Option<&str>)
                       -> Result<(Option<String>, Option<String>), PopenError> {
        let (out, err) = try!(self.communicate_bytes(input_data.map(|s| s.as_bytes())));
        let out_str = if let Some(out_vec) = out {
            Some(try!(String::from_utf8(out_vec)))
        } else { None };
        let err_str = if let Some(err_vec) = err {
            Some(try!(String::from_utf8(err_vec)))
        } else { None };
        Ok((out_str, err_str))
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


#[derive(Debug)]
pub enum PopenError {
    UtfError(FromUtf8Error),
    IoError(io::Error),
}

impl From<FromUtf8Error> for PopenError {
    fn from(err: FromUtf8Error) -> PopenError {
        PopenError::UtfError(err)
    }
}

impl From<io::Error> for PopenError {
    fn from(err: io::Error) -> PopenError {
        PopenError::IoError(err)
    }
}

impl Error for PopenError {
    fn description(&self) -> &str {
        match *self {
            PopenError::UtfError(ref err) => err.description(),
            PopenError::IoError(ref err) => err.description(),
        }
    }

    fn cause(&self) -> Option<&Error> {
        Some(match *self {
            PopenError::UtfError(ref err) => err as &Error,
            PopenError::IoError(ref err) => err as &Error,
        })
    }
}

impl fmt::Display for PopenError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            PopenError::UtfError(ref err) => fmt::Display::fmt(err, f),
            PopenError::IoError(ref err) => fmt::Display::fmt(err, f),
        }
    }
}
