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
pub use self::run::{Run, NullFile};
pub use self::pipeline::Pipeline;


mod run {
    use std::ffi::{OsStr, OsString};
    use std::io::{Result as IoResult, Read, Write};
    use std::fs::{File, OpenOptions};
    use std::ops::BitOr;

    use popen::{PopenConfig, Popen, Redirection, Result as PopenResult};
    use os_common::ExitStatus;

    use super::os::*;
    use super::Pipeline;

    #[derive(Debug)]
    pub struct Run {
        command: OsString,
        args: Vec<OsString>,
        config: PopenConfig,
        stdin_data: Option<Vec<u8>>,
    }

    impl Run {
        pub fn cmd<S: AsRef<OsStr>>(command: S) -> Run {
            Run {
                command: command.as_ref().to_owned(),
                args: vec![],
                config: PopenConfig::default(),
                stdin_data: None,
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

        pub fn stdin<T: IntoInputRedirection>(mut self, stdin: T) -> Run {
            match (&self.config.stdin, stdin.into_input_redirection()) {
                (&Redirection::None, InputRedirection::NoAction(new)) => self.config.stdin = new,
                (&Redirection::Pipe, InputRedirection::NoAction(Redirection::Pipe)) => (),
                (&Redirection::None, InputRedirection::FeedData(data)) => {
                    self.config.stdin = Redirection::Pipe;
                    self.stdin_data = Some(data);
                }
                (_, _) => panic!("stdin is already set"),
            }
            self
        }

        pub fn stdout<T: IntoOutputRedirection>(mut self, stdout: T) -> Run {
            match (&self.config.stdout, stdout.into_output_redirection()) {
                (&Redirection::None, new) => self.config.stdout = new,
                (&Redirection::Pipe, Redirection::Pipe) => (),
                (_, _) => panic!("stdout is already set"),
            }
            self
        }

        pub fn stderr<T: IntoOutputRedirection>(mut self, stderr: T) -> Run {
            match (&self.config.stderr, stderr.into_output_redirection()) {
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

        pub fn wait(self) -> PopenResult<ExitStatus> {
            self.popen()?.wait()
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

        pub fn capture(mut self) -> PopenResult<Capture> {
            let stdin_data = self.stdin_data.take();
            let mut p = self.popen()?;
            let (maybe_out, maybe_err) = p.communicate_bytes(
                stdin_data.as_ref().map(|v| &v[..]))?;
            let out = maybe_out.unwrap_or_else(Vec::new);
            let err = maybe_err.unwrap_or_else(Vec::new);
            let status = p.wait()?;
            Ok(Capture {
                stdout: out, stderr: err, exit_status: status
            })
        }
    }

    impl Clone for Run {
        fn clone(&self) -> Run {
            Run {
                command: self.command.clone(),
                args: self.args.clone(),
                config: self.config.try_clone().unwrap(),
                stdin_data: self.stdin_data.as_ref().cloned(),
            }
        }
    }

    impl BitOr for Run {
        type Output = Pipeline;

        fn bitor(self, rhs: Run) -> Pipeline {
            Pipeline::new(self, rhs)
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

    pub struct Capture {
        pub stdout: Vec<u8>,
        pub stderr: Vec<u8>,
        pub exit_status: ExitStatus,
    }

    impl Capture {
        pub fn stdout_str(&self) -> String {
            String::from_utf8_lossy(&self.stdout).into_owned()
        }

        pub fn stderr_str(&self) -> String {
            String::from_utf8_lossy(&self.stderr).into_owned()
        }
    }

    pub enum InputRedirection {
        NoAction(Redirection),
        FeedData(Vec<u8>),
    }

    pub trait IntoInputRedirection {
        fn into_input_redirection(self) -> InputRedirection;
    }

    impl IntoInputRedirection for Redirection {
        fn into_input_redirection(self) -> InputRedirection {
            if let Redirection::Merge = self {
                panic!("Redirection::Merge is only allowed for output streams");
            }
            InputRedirection::NoAction(self)
        }
    }

    impl IntoInputRedirection for File {
        fn into_input_redirection(self) -> InputRedirection {
            InputRedirection::NoAction(Redirection::File(self))
        }
    }

    pub struct NullFile;

    impl IntoInputRedirection for NullFile {
        fn into_input_redirection(self) -> InputRedirection {
            let null_file = OpenOptions::new().read(true).open(NULL_DEVICE).unwrap();
            InputRedirection::NoAction(Redirection::File(null_file))
        }
    }

    impl IntoInputRedirection for Vec<u8> {
        fn into_input_redirection(self) -> InputRedirection {
            InputRedirection::FeedData(self)
        }
    }

    impl<'a> IntoInputRedirection for &'a str {
        fn into_input_redirection(self) -> InputRedirection {
            InputRedirection::FeedData(self.as_bytes().to_vec())
        }
    }

    pub trait IntoOutputRedirection {
        fn into_output_redirection(self) -> Redirection;
    }

    impl IntoOutputRedirection for Redirection {
        fn into_output_redirection(self) -> Redirection {
            self
        }
    }

    impl IntoOutputRedirection for File {
        fn into_output_redirection(self) -> Redirection {
            Redirection::File(self)
        }
    }

    impl IntoOutputRedirection for NullFile {
        fn into_output_redirection(self) -> Redirection {
            let null_file = OpenOptions::new().write(true).open(NULL_DEVICE).unwrap();
            Redirection::File(null_file)
        }
    }
}


mod pipeline {
    use std::io::{Result as IoResult, Read, Write};
    use std::ops::BitOr;
    use std::fs::File;

    use popen;
    use popen::{Popen, Redirection, Result as PopenResult};
    use os_common::ExitStatus;

    use super::run::{Run, IntoInputRedirection, InputRedirection, IntoOutputRedirection};

    #[derive(Debug)]
    pub struct Pipeline {
        cmds: Vec<Run>,
        stdin: Redirection,
        stdout: Redirection,
        stdin_data: Option<Vec<u8>>,
    }

    impl Pipeline {
        pub fn new(cmd1: Run, cmd2: Run) -> Pipeline {
            Pipeline {
                cmds: vec![cmd1, cmd2],
                stdin: Redirection::None,
                stdout: Redirection::None,
                stdin_data: None,
            }
        }

        pub fn add(mut self, r: Run) -> Pipeline {
            self.cmds.push(r);
            self
        }

        pub fn stdin<T: IntoInputRedirection>(mut self, stdin: T) -> Pipeline {
            match stdin.into_input_redirection() {
                InputRedirection::NoAction(r) => self.stdin = r,
                InputRedirection::FeedData(data) => {
                    self.stdin = Redirection::Pipe;
                    self.stdin_data = Some(data);
                }
            };
            self
        }

        pub fn stdout<T: IntoOutputRedirection>(mut self, stdout: T) -> Pipeline {
            self.stdout = stdout.into_output_redirection();
            self
        }

        // Terminators:

        pub fn popen(mut self) -> PopenResult<Vec<Popen>> {
            assert!(self.cmds.len() >= 2);
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

        pub fn wait(self) -> PopenResult<ExitStatus> {
            let mut v = self.popen()?;
            // Waiting on a pipeline waits for all commands, but
            // returns the status of the last one.  This is how the
            // shells do it.  If the caller needs more precise control
            // over which status is returned, they can call popen().
            v.last_mut().unwrap().wait()
        }

        pub fn stream_stdout(self) -> PopenResult<Box<Read>> {
            let v = self.stdout(Redirection::Pipe).popen()?;
            Ok(Box::new(ReadPipelineAdapter(v)))
        }

        pub fn stream_stdin(self) -> PopenResult<Box<Write>> {
            let v = self.stdin(Redirection::Pipe).popen()?;
            Ok(Box::new(WritePipelineAdapter(v)))
        }

        pub fn capture(mut self) -> PopenResult<CaptureOutput> {
            assert!(self.cmds.len() >= 2);

            let stdin_data = self.stdin_data.take();
            let mut v = self.stdout(Redirection::Pipe).popen()?;

            let mut first = v.drain(..1).next().unwrap();
            let vlen = v.len();
            let mut last = v.drain(vlen - 1..).next().unwrap();

            let (maybe_out, _) = popen::communicate_bytes(
                &mut first.stdin, &mut last.stdout, &mut None,
                stdin_data.as_ref().map(|v| &v[..]))?;
            let out = maybe_out.unwrap_or_else(Vec::new);

            let status = last.wait()?;

            Ok(CaptureOutput { stdout: out, exit_status: status })
        }
    }

    impl Clone for Pipeline {
        fn clone(&self) -> Pipeline {
            Pipeline {
                cmds: self.cmds.clone(),
                stdin: self.stdin.try_clone().unwrap(),
                stdout: self.stdout.try_clone().unwrap(),
                stdin_data: self.stdin_data.clone()
            }
        }
    }

    impl BitOr<Run> for Pipeline {
        type Output = Pipeline;

        fn bitor(self, rhs: Run) -> Pipeline {
            self.add(rhs)
        }
    }

    impl BitOr for Pipeline {
        type Output = Pipeline;

        fn bitor(mut self, rhs: Pipeline) -> Pipeline {
            self.cmds.extend(rhs.cmds);
            self.stdout = rhs.stdout;
            self
        }
    }

    #[derive(Debug)]
    struct ReadPipelineAdapter(Vec<Popen>);

    impl Read for ReadPipelineAdapter {
        fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
            let last = self.0.last_mut().unwrap();
            last.stdout.as_mut().unwrap().read(buf)
        }
    }

    #[derive(Debug)]
    struct WritePipelineAdapter(Vec<Popen>);

    impl WritePipelineAdapter {
        fn stdin(&mut self) -> &mut File {
            let first = self.0.first_mut().unwrap();
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

    pub struct CaptureOutput {
        pub stdout: Vec<u8>,
        pub exit_status: ExitStatus
    }

    impl CaptureOutput {
        pub fn stdout_str(&self) -> String {
            String::from_utf8_lossy(&self.stdout).into_owned()
        }
    }
}
