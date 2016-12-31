use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};

use popen::{PopenConfig, Popen, PopenError, Redirection};

pub struct Popt {
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

    pub fn stdin<T: IntoRedirection>(mut self, stdin: T) -> Popt {
        self.config.stdin = stdin.into_redirection(false);
        self
    }

    pub fn stdout<T: IntoRedirection>(mut self, stdout: T) -> Popt {
        self.config.stdout = stdout.into_redirection(true);
        self
    }

    pub fn stderr<T: IntoRedirection>(mut self, stderr: T) -> Popt {
        self.config.stderr = stderr.into_redirection(true);
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
