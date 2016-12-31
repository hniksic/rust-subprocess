use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{Result as IoResult, Read, Write};

use popen::{PopenConfig, Popen, PopenError, Redirection};

pub struct Run {
    command: OsString,
    args: Vec<OsString>,
    config: PopenConfig,
    detached: bool,
}

#[cfg(unix)]
pub const NULL_DEVICE: &'static str = "/dev/null";

#[cfg(windows)]
pub const NULL_DEVICE: &'static str = "nul";

pub trait IntoRedirection {
    fn into_redirection(self, bool) -> Redirection;
}

impl IntoRedirection for Redirection {
    fn into_redirection(self, output: bool) -> Redirection {
        if !output {
            if let Redirection::Merge = self {
                panic!("Redirection::Merge is only allowed for output streams");
            }
        }
        self
    }
}

impl IntoRedirection for File {
    fn into_redirection(self, _output: bool) -> Redirection {
        Redirection::File(self)
    }
}

pub struct NullFile;

impl IntoRedirection for NullFile {
    fn into_redirection(self, output: bool) -> Redirection {
        let null_file = if output {
            OpenOptions::new().write(true).open(NULL_DEVICE)
        } else {
            OpenOptions::new().read(true).open(NULL_DEVICE)
        }.unwrap();
        Redirection::File(null_file)
    }
}

impl Run {
    pub fn new<S: AsRef<OsStr>>(command: S) -> Run {
        Run {
            command: command.as_ref().to_owned(),
            args: vec![],
            config: PopenConfig::default(),
            detached: false,
        }
    }

    pub fn arg<S: AsRef<OsStr>>(mut self, arg: S) -> Run {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<S: AsRef<OsStr>>(mut self, args: &[S]) -> Run {
        self.args.extend(args.iter().map(|x| x.as_ref().to_owned()));
        self
    }

    pub fn detached(mut self) -> Run {
        self.detached = true;
        self
    }

    pub fn stdin<T: IntoRedirection>(mut self, stdin: T) -> Run {
        self.config.stdin = stdin.into_redirection(false);
        self
    }

    pub fn stdout<T: IntoRedirection>(mut self, stdout: T) -> Run {
        self.config.stdout = stdout.into_redirection(true);
        self
    }

    pub fn stderr<T: IntoRedirection>(mut self, stderr: T) -> Run {
        self.config.stderr = stderr.into_redirection(true);
        self
    }

    // Terminators

    pub fn popen(mut self) -> Result<Popen, PopenError> {
        self.args.insert(0, self.command);
        let mut p = Popen::create(&self.args, self.config)?;
        if self.detached {
            p.detach();
        }
        Ok(p)
    }

    pub fn stream_stdout(self) -> Result<Box<Read>, PopenError> {
        if let Redirection::Pipe = self.config.stdout {}
        else {
            panic!("cannot read from non-redirected stdout");
        }
        let p = self.popen()?;
        Ok(Box::new(ReadOutAdapter(p)))
    }

    pub fn stream_stderr(self) -> Result<Box<Read>, PopenError> {
        if let Redirection::Pipe = self.config.stderr {}
        else {
            panic!("cannot read from non-redirected stderr");
        }
        let p = self.popen()?;
        Ok(Box::new(ReadErrAdapter(p)))
    }

    pub fn stream_stdin(self) -> Result<Box<Write>, PopenError> {
        if let Redirection::Pipe = self.config.stdin {}
        else {
            panic!("cannot write to non-redirected stdin");
        }
        let p = self.popen()?;
        Ok(Box::new(WriteAdapter(p)))
    }
}

struct ReadOutAdapter(Popen);

impl Read for ReadOutAdapter {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.0.stdout.as_mut().unwrap().read(buf)
    }
}

struct ReadErrAdapter(Popen);

impl Read for ReadErrAdapter {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.0.stderr.as_mut().unwrap().read(buf)
    }
}

struct WriteAdapter(Popen);

impl Write for WriteAdapter {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        self.0.stdin.as_mut().unwrap().write(buf)
    }
    fn flush(&mut self) -> IoResult<()> {
        self.0.stdin.as_mut().unwrap().flush()
    }
}

// We must implement Drop in order to close the stream.  The typical
// use case for stream_stdin() is a process that reads something from
// stdin.  WriteAdapter going out of scope invokes Popen::drop(),
// which waits for the process to exit.  Without closing stdin, this
// deadlocks because the child process hangs reading its stdin.

impl Drop for WriteAdapter {
    fn drop(&mut self) {
        self.0.stdin.take();
    }
}
