use std::ffi::{OsStr, OsString};

use popen::{PopenConfig, Popen, PopenError, Redirection};

pub struct Popt {
    command: OsString,
    args: Vec<OsString>,
    config: PopenConfig,
    detached: bool,
}

impl Popt {
    pub fn new<S: AsRef<OsStr>>(command: S) -> Popt {
        Popt {
            command: command.as_ref().to_owned(),
            args: vec![],
            config: PopenConfig::default(),
            detached: false,
        }
    }

    pub fn arg<S: AsRef<OsStr>>(mut self, arg: S) -> Popt {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<S: AsRef<OsStr>>(mut self, args: &[S]) -> Popt {
        self.args.extend(args.iter().map(|x| x.as_ref().to_owned()));
        self
    }

    pub fn detached(mut self) -> Popt {
        self.detached = true;
        self
    }

    pub fn stdin(mut self, stdin: Redirection) -> Popt {
        self.config.stdin = stdin;
        self
    }

    pub fn stdout(mut self, stdout: Redirection) -> Popt {
        self.config.stdout = stdout;
        self
    }

    pub fn stderr(mut self, stderr: Redirection) -> Popt {
        self.config.stderr = stderr;
        self
    }

    pub fn spawn(mut self) -> Result<Popen, PopenError> {
        self.args.insert(0, self.command);
        let mut p = Popen::create(&self.args, self.config)?;
        if self.detached {
            p.detach();
        }
        Ok(p)
    }
}
