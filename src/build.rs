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
mod os {
    pub const NULL_DEVICE: &'static str = "/dev/null";
    pub const SHELL: [&'static str; 2] = ["sh", "-c"];
}

#[cfg(windows)]
mod os {
    pub const NULL_DEVICE: &'static str = "nul";
    pub const SHELL: [&'static str; 2] = ["cmd.exe", "/c"];
}

pub use self::os::*;

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
    pub fn cmd<S: AsRef<OsStr>>(command: S) -> Run {
        Run {
            command: command.as_ref().to_owned(),
            args: vec![],
            config: PopenConfig::default(),
        }
    }

    pub fn shell<S: AsRef<OsStr>>(cmdstr: S) -> Run {
        Run::cmd(SHELL[0]).args(&SHELL[1..]).arg(cmdstr)
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
        match (&self.config.stdin, stdin.into_redirection(false)) {
            (&Redirection::None, new) => self.config.stdin = new,
            (&Redirection::Pipe, Redirection::Pipe) => (),
            (_, _) => panic!("stdin is already set"),
        }
        self
    }

    pub fn stdout<T: IntoRedirection>(mut self, stdout: T) -> Run {
        match (&self.config.stdout, stdout.into_redirection(true)) {
            (&Redirection::None, new) => self.config.stdout = new,
            (&Redirection::Pipe, Redirection::Pipe) => (),
            (_, _) => panic!("stdout is already set"),
        }
        self
    }

    pub fn stderr<T: IntoRedirection>(mut self, stderr: T) -> Run {
        match (&self.config.stderr, stderr.into_redirection(true)) {
            (&Redirection::None, new) => self.config.stderr = new,
            (&Redirection::Pipe, Redirection::Pipe) => (),
            (_, _) => panic!("stderr is already set"),
        }
        self
    }

    // Terminators

    pub fn popen(mut self) -> PopenResult<Popen> {
        self.args.insert(0, self.command);
        let p = Popen::create(&self.args, self.config)?;
        Ok(p)
    }

    pub fn stream_stdout(self) -> PopenResult<Box<Read>> {
        let p = self.stdout(Redirection::Pipe).popen()?;
        Ok(Box::new(ReadOutAdapter(p)))
    }

    pub fn stream_stderr(self) -> PopenResult<Box<Read>> {
        let p = self.stderr(Redirection::Pipe).popen()?;
        Ok(Box::new(ReadErrAdapter(p)))
    }

    pub fn stream_stdin(self) -> PopenResult<Box<Write>> {
        let p = self.stdin(Redirection::Pipe).popen()?;
        Ok(Box::new(WriteAdapter(p)))
    }
}

impl Clone for Run {
    fn clone(&self) -> Run {
        Run {
            command: self.command.clone(),
            args: self.args.clone(),
            config: self.config.try_clone().unwrap(),
        }
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
        let mut v = self.stdout(Redirection::Pipe).popen()?;
        let vlen = v.len();
        let last = v.drain(vlen - 1..).next().unwrap();
        Ok(Box::new(ReadOutAdapter(last)))
    }

    pub fn stream_stdin(self) -> PopenResult<Box<Write>> {
        let v = self.stdin(Redirection::Pipe).popen()?;
        Ok(Box::new(WritePipelineAdapter(v)))
    }
}

impl Clone for Pipeline {
    fn clone(&self) -> Pipeline {
        Pipeline {
            cmds: self.cmds.clone(),
            stdin: self.stdin.try_clone().unwrap(),
            stdout: self.stdout.try_clone().unwrap(),
        }
    }
}

impl BitOr<Run> for Pipeline {
    type Output = Pipeline;

    fn bitor(self, rhs: Run) -> Pipeline {
        self.add(rhs)
    }
}

#[derive(Debug)]
struct WritePipelineAdapter(Vec<Popen>);

impl WritePipelineAdapter {
    fn stdin(&mut self) -> &mut File {
        let ref mut first = self.0[0];
        first.stdin.as_mut().unwrap()
    }
}

impl Write for WritePipelineAdapter {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        self.stdin().write(buf)
    }
    fn flush(&mut self) -> IoResult<()> {
        self.stdin().flush()
    }
}

impl Drop for WritePipelineAdapter {
    // the same rationale as Drop for WriteAdapter
    fn drop(&mut self) {
        let ref mut first = self.0[0];
        first.stdin.take();
    }
}
