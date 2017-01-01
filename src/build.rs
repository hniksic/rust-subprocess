use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{Result as IoResult, Read, Write};

use popen::{PopenConfig, Popen, Redirection, Result as PopenResult};
use std::ops::BitOr;

#[derive(Debug)]
pub struct Run {
    command: OsString,
    args: Vec<OsString>,
    config: PopenConfig,
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
        self.config.detached = true;
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

    pub fn popen(mut self) -> PopenResult<Popen> {
        self.args.insert(0, self.command);
        let p = Popen::create(&self.args, self.config)?;
        Ok(p)
    }

    pub fn stream_stdout(self) -> PopenResult<Box<Read>> {
        if let Redirection::Pipe = self.config.stdout {}
        else {
            panic!("cannot read from non-redirected stdout");
        }
        let p = self.popen()?;
        Ok(Box::new(ReadOutAdapter(p)))
    }

    pub fn stream_stderr(self) -> PopenResult<Box<Read>> {
        if let Redirection::Pipe = self.config.stderr {}
        else {
            panic!("cannot read from non-redirected stderr");
        }
        let p = self.popen()?;
        Ok(Box::new(ReadErrAdapter(p)))
    }

    pub fn stream_stdin(self) -> PopenResult<Box<Write>> {
        if let Redirection::Pipe = self.config.stdin {}
        else {
            panic!("cannot write to non-redirected stdin");
        }
        let p = self.popen()?;
        Ok(Box::new(WriteAdapter(p)))
    }
}

impl BitOr for Run {
    type Output = Pipeline;

    fn bitor(self, rhs: Run) -> Pipeline {
        Pipeline::new().add(self).add(rhs)
    }
}

#[derive(Debug)]
struct ReadOutAdapter(Popen);

impl Read for ReadOutAdapter {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.0.stdout.as_mut().unwrap().read(buf)
    }
}

#[derive(Debug)]
struct ReadErrAdapter(Popen);

impl Read for ReadErrAdapter {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.0.stderr.as_mut().unwrap().read(buf)
    }
}

#[derive(Debug)]
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


#[derive(Debug)]
pub struct Pipeline {
    cmds: Vec<Run>,
    stdin: Redirection,
    stdout: Redirection,
}

impl Pipeline {
    pub fn new() -> Pipeline {
        Pipeline {
            cmds: Vec::new(),
            stdin: Redirection::None,
            stdout: Redirection::None,
        }
    }

    pub fn add(mut self, r: Run) -> Pipeline {
        self.cmds.push(r);
        self
    }

    pub fn stdin<T: IntoRedirection>(mut self, stdin: T) -> Pipeline {
        self.stdin = stdin.into_redirection(false);
        self
    }

    pub fn stdout<T: IntoRedirection>(mut self, stdout: T) -> Pipeline {
        self.stdout = stdout.into_redirection(true);
        self
    }

    // Terminators:

    pub fn popen(mut self) -> PopenResult<Vec<Popen>> {
        if self.cmds.is_empty() {
            panic!("empty pipeline");
        }
        let cnt = self.cmds.len();

        let first_cmd = self.cmds.drain(..1).next().unwrap();
        self.cmds.insert(0, first_cmd.stdin(self.stdin));

        let last_cmd = self.cmds.drain(cnt - 1..).next().unwrap();
        self.cmds.push(last_cmd.stdout(self.stdout));

        let mut ret = Vec::<Popen>::new();

        for (idx, mut runner) in self.cmds.into_iter().enumerate() {
            if idx != 0 {
                let prev_stdout = ret[idx - 1].stdout.take().unwrap();
                runner = runner.stdin(prev_stdout);
            }
            if idx != cnt - 1 {
                runner = runner.stdout(Redirection::Pipe);
            }
            ret.push(runner.popen()?);
        }
        Ok(ret)
    }

    pub fn stream_stdout(self) -> PopenResult<Box<Read>> {
        if let Redirection::Pipe = self.stdout {}
        else {
            panic!("cannot read from non-redirected stdout");
        }
        let mut v = self.popen()?;
        let vlen = v.len();
        let last = v.drain(vlen - 1..).next().unwrap();
        Ok(Box::new(ReadOutAdapter(last)))
    }

    pub fn stream_stdin(self) -> PopenResult<Box<Write>> {
        if let Redirection::Pipe = self.stdin {}
        else {
            panic!("cannot write to non-redirected stdin");
        }
        let mut v = self.popen()?;
        let vlen = v.len();
        let last = v.drain(vlen - 1..).next().unwrap();
        Ok(Box::new(WriteAdapter(last)))
    }
}

impl BitOr<Run> for Pipeline {
    type Output = Pipeline;

    fn bitor(self, rhs: Run) -> Pipeline {
        self.add(rhs)
    }
}
