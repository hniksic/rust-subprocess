extern crate crossbeam;

use std::error::Error;
use std::path::{PathBuf, Path};
use std::io;
use std::io::{Read, Write};
use std::fs::File;
use std::string::FromUtf8Error;
use std::fmt;

use subprocess::common::ExitStatus;

#[derive(Debug)]
pub struct Popen {
    pid: Option<u32>,
    exit_status: Option<ExitStatus>,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,

    ext_data: os::ExtPopenData,
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
            ext_data: os::ExtPopenData::default(),
        };
        inst.start(args, stdin, stdout, stderr)?;
        Ok(inst)
    }

    pub fn create<P: AsRef<Path>>(args: &[P]) -> io::Result<Popen> {
        Popen::create_full(args, Redirection::None, Redirection::None, Redirection::None)
    }

    pub fn detach(&mut self) {
        self.pid = None;
    }

    fn make_child_streams(&mut self, stdin: Redirection, stdout: Redirection, stderr: Redirection)
                          -> io::Result<(Option<File>, Option<File>, Option<File>)> {
        let child_stdin = match stdin {
            Redirection::Pipe => {
                let (read, mut write) = os::make_pipe()?;
                os::set_inheritable(&mut write, false)?;
                self.stdin = Some(write);
                Some(read)
            }
            Redirection::File(mut file) => {
                os::set_inheritable(&mut file, true)?;
                Some(file)
            }
            Redirection::None => None,
        };
        let child_stdout = match stdout {
            Redirection::Pipe => {
                let (mut read, write) = os::make_pipe()?;
                os::set_inheritable(&mut read, false)?;
                self.stdout = Some(read);
                Some(write)
            }
            Redirection::File(mut file) => {
                os::set_inheritable(&mut file, true)?;
                Some(file)
            }
            Redirection::None => None
        };
        let child_stderr = match stderr {
            Redirection::Pipe => {
                let (mut read, write) = os::make_pipe()?;
                os::set_inheritable(&mut read, false)?;
                self.stderr = Some(read);
                Some(write)
            }
            Redirection::File(mut file) => {
                os::set_inheritable(&mut file, true)?;
                Some(file)
            }
            Redirection::None => None
        };
        Ok((child_stdin, child_stdout, child_stderr))
    }

    fn read_chunk(f: &mut File, append_to: &mut Vec<u8>) -> io::Result<bool> {
        let mut buf = [0u8; 8192];
        let cnt = f.read(&mut buf)?;
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
            while Popen::read_chunk(outfile, &mut contents)? {
            }
        }
        outfile.take();
        Ok(contents)
    }

    fn comm_write(infile: &mut Option<File>, input_data: &[u8]) -> io::Result<()> {
        {
            let infile = infile.as_mut().expect("file missing");
            infile.write_all(input_data)?;
        }
        infile.take();
        Ok(())
    }

    pub fn communicate_bytes(&mut self, input_data: Option<&[u8]>)
                             -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        match (&mut self.stdin, &mut self.stdout, &mut self.stderr) {
            (mut stdin_ref @ &mut Some(_), &mut None, &mut None) => {
                let input_data = input_data.expect("must provide input to redirected stdin");
                Popen::comm_write(stdin_ref, input_data)?;
                Ok((None, None))
            }
            (&mut None, mut stdout_ref @ &mut Some(_), &mut None) => {
                assert!(input_data.is_none(), "cannot provide input to non-redirected stdin");
                let out = Popen::comm_read(stdout_ref)?;
                Ok((Some(out), None))
            }
            (&mut None, &mut None, mut stderr_ref @ &mut Some(_)) => {
                assert!(input_data.is_none(), "cannot provide input to non-redirected stdin");
                let err = Popen::comm_read(stderr_ref)?;
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
                        Popen::comm_write(stdin_ref, input_data)?;
                    }
                    Ok((if let Some(out_thr) = out_thr {Some(out_thr.join()?)} else {None},
                        if let Some(err_thr) = err_thr {Some(err_thr.join()?)} else {None}))
                })
        }
    }

    pub fn communicate(&mut self, input_data: Option<&str>)
                       -> Result<(Option<String>, Option<String>), PopenError> {
        let (out, err) = self.communicate_bytes(input_data.map(|s| s.as_bytes()))?;
        let out_str = if let Some(out_vec) = out {
            Some(String::from_utf8(out_vec)?)
        } else { None };
        let err_str = if let Some(err_vec) = err {
            Some(String::from_utf8(err_vec)?)
        } else { None };
        Ok((out_str, err_str))
    }

    pub fn get_pid(&self) -> Option<u32> {
        self.pid
    }

    fn start(&mut self,
             args: Vec<PathBuf>,
             stdin: Redirection, stdout: Redirection, stderr: Redirection)
             -> io::Result<()> {
        (self as &mut PopenOs).start(args, stdin, stdout, stderr)
    }

    pub fn wait(&mut self) -> Result<ExitStatus, PopenError> {
        (self as &mut PopenOs).wait()
    }

    pub fn poll(&mut self) -> Option<ExitStatus> {
        (self as &mut PopenOs).poll()
    }

    pub fn terminate(&self) -> io::Result<()> {
        (self as &PopenOs).terminate()
    }

    pub fn kill(&self) -> io::Result<()> {
        (self as &PopenOs).kill()
    }
}


trait PopenOs {
    fn start(&mut self, args: Vec<PathBuf>,
             stdin: Redirection, stdout: Redirection, stderr: Redirection)
             -> io::Result<()>;
    fn wait(&mut self) -> Result<ExitStatus, PopenError>;
    fn poll(&mut self) -> Option<ExitStatus>;
    fn terminate(&self) -> io::Result<()>;
    fn kill(&self) -> io::Result<()>;

}

#[cfg(unix)]
mod os {
    use super::*;
    use std::io;
    use std::io::{Read, Write};
    use std::fs::File;
    use std::path::PathBuf;
    use subprocess::posix;
    use std::mem;
    use std::os::unix::io::AsRawFd;
    use subprocess::common::ExitStatus;

    pub type ExtPopenData = ();

    impl super::PopenOs for Popen {
        fn start(&mut self,
                 args: Vec<PathBuf>,
                 stdin: Redirection, stdout: Redirection, stderr: Redirection)
                 -> io::Result<()> {
            let mut exec_fail_pipe = posix::pipe()?;
            set_inheritable(&mut exec_fail_pipe.0, false)?;
            set_inheritable(&mut exec_fail_pipe.1, false)?;
            {
                let child_ends = self.make_child_streams(stdin, stdout, stderr)?;
                let child_pid = posix::fork()?;
                if child_pid == 0 {
                    mem::drop(exec_fail_pipe.0);
                    let result: io::Result<()> = self.do_exec(args, child_ends);
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
            exec_fail_pipe.0.read_to_string(&mut error_string)?;
            if error_string.len() != 0 {
                let error_code: i32 = error_string.parse()
                    .expect("parse child error code");
                Err(io::Error::from_raw_os_error(error_code))
            } else {
                Ok(())
            }
        }

        fn wait(&mut self) -> Result<ExitStatus, PopenError> {
            while let None = self.exit_status {
                self.waitpid(0)?;
            }
            Ok(self.exit_status.unwrap())
        }

        fn poll(&mut self) -> Option<ExitStatus> {
            match self.waitpid(posix::WNOHANG) {
                Ok(_) => self.exit_status,
                Err(_) => None
            }
        }

        fn terminate(&self) -> io::Result<()> {
            self.send_signal(posix::SIGTERM)
        }

        fn kill(&self) -> io::Result<()> {
            self.send_signal(posix::SIGKILL)
        }
    }

    trait PopenOsImpl: super::PopenOs {
        fn do_exec(&self, args: Vec<PathBuf>,
                   child_ends: (Option<File>, Option<File>, Option<File>)) -> io::Result<()>;
        fn waitpid(&mut self, flags: i32) -> io::Result<()>;
        fn send_signal(&self, signal: u8) -> io::Result<()>;
    }

    impl PopenOsImpl for Popen {
        fn do_exec(&self, args: Vec<PathBuf>,
                   child_ends: (Option<File>, Option<File>, Option<File>)) -> io::Result<()> {
            let (stdin, stdout, stderr) = child_ends;
            if let Some(stdin) = stdin {
                posix::dup2(stdin.as_raw_fd(), 0)?;
            }
            if let Some(stdout) = stdout {
                posix::dup2(stdout.as_raw_fd(), 1)?;
            }
            if let Some(stderr) = stderr {
                posix::dup2(stderr.as_raw_fd(), 2)?;
            }
            posix::execvp(&args[0], &args)
        }

        fn waitpid(&mut self, flags: i32) -> io::Result<()> {
            match self.pid {
                Some(pid) => {
                    // XXX handle some kinds of error - at least ECHILD and EINTR
                    let (pid_out, exit_status) = posix::waitpid(pid, flags)?;
                    if pid_out == pid {
                        self.pid = None;
                        self.exit_status = Some(exit_status);
                    }
                },
                None => (),
            }
            Ok(())
        }

        fn send_signal(&self, signal: u8) -> io::Result<()> {
            match self.pid {
                Some(pid) => {
                    posix::kill(pid, signal)
                },
                None => Ok(()),
            }
        }
    }

    pub fn set_inheritable(f: &mut File, inheritable: bool) -> io::Result<()> {
        if inheritable {
            // Unix pipes are inheritable by default.
        } else {
            let fd = f.as_raw_fd();
            let old = posix::fcntl(fd, posix::F_GETFD, None)?;
            posix::fcntl(fd, posix::F_SETFD, Some(old | posix::FD_CLOEXEC))?;
        }
        Ok(())
    }

    pub fn make_pipe() -> io::Result<(File, File)> {
        posix::pipe()
    }
}


#[cfg(windows)]
mod os {
    use super::*;
    use std::io;
    use std::fs::File;
    use std::path::PathBuf;
    use subprocess::win32;
    use subprocess::common::ExitStatus;
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    #[derive(Debug, Default)]
    pub struct ExtPopenData {
        handle: Option<win32::Handle>,
    }

    impl super::PopenOs for Popen {
        fn start(&mut self,
                 args: Vec<PathBuf>,
                 stdin: Redirection, stdout: Redirection, stderr: Redirection)
                 -> io::Result<()> {
            let (child_stdin, child_stdout, child_stderr)
                = self.make_child_streams(stdin, stdout, stderr)?;
            let cmdline = assemble_cmdline(args)?;
            let (handle, pid)
                = win32::CreateProcess(&cmdline, true, 0,
                                       child_stdin, child_stdout, child_stderr,
                                       win32::STARTF_USESTDHANDLES)?;
            self.pid = Some(pid as u32);
            self.ext_data.handle = Some(handle);
            Ok(())
        }

        fn wait(&mut self) -> Result<ExitStatus, PopenError> {
            self.wait_handle(None)?;
            match self.exit_status {
                Some(exit_status) => Ok(exit_status),
                // Since we invoked wait_handle without timeout, exit status should
                // exist at this point.  The only way for it not to exist would be if
                // something strange happened, like WaitForSingleObject returneing
                // something other than OBJECT_0.
                None => Err(PopenError::LogicError("Failed to obtain exit status"))
            }
        }

        fn poll(&mut self) -> Option<ExitStatus> {
            match self.wait_handle(Some(0.0)) {
                Ok(_) => self.exit_status,
                Err(_) => None
            }
        }

        fn terminate(&self) -> io::Result<()> {
            panic!();
        }

        fn kill(&self) -> io::Result<()> {
            panic!();
        }
    }

    trait PopenOsImpl: super::PopenOs {
        fn wait_handle(&mut self, timeout: Option<f64>) -> io::Result<Option<ExitStatus>>;
    }

    impl PopenOsImpl for Popen {
        fn wait_handle(&mut self, timeout: Option<f64>) -> io::Result<Option<ExitStatus>> {
            if self.ext_data.handle.is_some() {
                let timeout = timeout.map(|t| (t * 1000.0) as u32);
                let event = win32::WaitForSingleObject(
                    self.ext_data.handle.as_ref().unwrap(), timeout)?;
                if let win32::WaitEvent::OBJECT_0 = event {
                    self.pid = None;
                    let handle = self.ext_data.handle.take().unwrap();
                    let exit_code = win32::GetExitCodeProcess(&handle)?;
                    self.exit_status = Some(ExitStatus::Exited(exit_code));
                }
            }
            Ok(self.exit_status)
        }
    }

    pub fn set_inheritable(f: &mut File, inheritable: bool) -> io::Result<()> {
        win32::SetHandleInformation(f, win32::HANDLE_FLAG_INHERIT,
                                         if inheritable {1} else {0})?;
        Ok(())
    }

    pub fn make_pipe() -> io::Result<(File, File)> {
        win32::CreatePipe(true)
    }

    fn assemble_cmdline(args: Vec<PathBuf>) -> io::Result<OsString> {
        let mut cmdline = Vec::<u16>::new();
        for arg in args {
            if arg.as_os_str().encode_wide().any(|c| c == 0) {
                return Err(io::Error::from_raw_os_error(win32::ERROR_BAD_PATHNAME as i32));
            }
            append_quoted(arg.as_os_str(), &mut cmdline);
            cmdline.push(' ' as u16);
        }
        Ok(OsString::from_wide(&cmdline))
    }

    // Translated from ArgvQuote at http://tinyurl.com/zmgtnls
    fn append_quoted(arg: &OsStr, cmdline: &mut Vec<u16>) {
        if !arg.is_empty() && !arg.encode_wide().any(
            |c| c == ' ' as u16 || c == '\t' as u16 || c == '\n' as u16 ||
                c == '\x0b' as u16 || c == '\"' as u16) {
            cmdline.extend(arg.encode_wide());
            return
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
                for _ in 0..num_backslashes*2 {
                    cmdline.push('\\' as u16);
                }
                break;
            } else if arg[i] == b'"' as u16 {
                for _ in 0..num_backslashes*2 + 1 {
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


impl Drop for Popen {
    // Wait for the process to exit.  To avoid the wait, call
    // detach().
    fn drop(&mut self) {
        // drop() is invoked if a try! fails during construction, in which
        // case wait() would panic because an exit status cannot be obtained.
        if self.exit_status.is_some() {
            // XXX Log error occurred during wait()?
            self.wait().ok();
        }
    }
}


#[derive(Debug)]
pub enum PopenError {
    UtfError(FromUtf8Error),
    IoError(io::Error),
    LogicError(&'static str),
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
            PopenError::LogicError(description) => description,
        }
    }

    fn cause(&self) -> Option<&Error> {
        match *self {
            PopenError::UtfError(ref err) => Some(err as &Error),
            PopenError::IoError(ref err) => Some(err as &Error),
            PopenError::LogicError(_) => None,
        }
    }
}

impl fmt::Display for PopenError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            PopenError::UtfError(ref err) => fmt::Display::fmt(err, f),
            PopenError::IoError(ref err) => fmt::Display::fmt(err, f),
            PopenError::LogicError(desc) => f.write_str(desc)
        }
    }
}
