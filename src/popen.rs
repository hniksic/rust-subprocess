use std::path::{PathBuf, Path};
use std::io::{Result, Error, Read, Write};
use std::mem;

#[derive(Debug)]
pub struct Popen {
    args: Vec<PathBuf>,
    pid: Option<u32>,
    return_code: Option<u8>,
}

mod wrapped {
    use std::io::{Result, Error};
    use std::path::Path;
    use libc;
    use std::os::unix::ffi::OsStrExt;
    use std::fs::File;
    use std::os::unix::io::FromRawFd;
    use std::ptr;

    fn check_err<T: Ord + Default>(num: T) -> Result<T> {
        if num < T::default() {
            return Err(Error::last_os_error());
        }
        Ok(num)
    }

    fn path_as_ptr(p: &Path) -> *const libc::c_char {
        let c_bytes = p.as_os_str().as_bytes();
        &c_bytes[0] as *const u8 as *const libc::c_char
    }

    pub fn pipe() -> Result<(File, File)> {
        let mut fds = [0 as libc::c_int; 2];
        try!(check_err(unsafe { libc::pipe(&mut fds[0]) }));
        Ok(unsafe {
            (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1]))
        })
    }

    pub fn fork() -> Result<u32> {
        check_err(unsafe { libc::fork() }).map(|pid| pid as u32)
    }

    pub fn execvp<P1, P2>(cmd: P1, args: &[P2]) -> Result<()>
        where P1: AsRef<Path>, P2: AsRef<Path> {
        let mut args_os: Vec<_> = args.iter()
            .map(|x| path_as_ptr(x.as_ref())).collect();
        args_os.push(ptr::null());
        let argv = &args_os[0] as *const *const libc::c_char;
        check_err(unsafe { libc::execvp(path_as_ptr(cmd.as_ref()), argv) })
            .map(|_| ())
    }

    pub fn _exit(status: u8) -> ! {
        unsafe { libc::_exit(status as libc::c_int) }
    }
}

impl Popen {
    pub fn create<P: AsRef<Path>>(args: &[P]) -> Result<Popen> {
        let args: Vec<PathBuf> = args.iter()
            .map(|p| p.as_ref().to_owned()).collect();
        let mut inst = Popen {
            args: args,
            pid: None,
            return_code: None,
        };
        try!(inst.start());
        Ok(inst)
    }

    fn start(&mut self) -> Result<()> {
        let mut exec_fail_pipe = try!(wrapped::pipe());
        let child_pid = try!(wrapped::fork());
        if child_pid == 0 {
            mem::drop(exec_fail_pipe.0);
            let result = wrapped::execvp(&self.args[0], &self.args);
            let error_code: i32 = match result {
                Ok(()) => unreachable!(),
                Err(e) => e.raw_os_error().unwrap_or(-1)
            };
            // XXX we don't really need formatting here - just send
            // off the 4-byte content of the variable over the pipe
            exec_fail_pipe.1.write_all(format!("{}", error_code).as_bytes()).unwrap();
            wrapped::_exit(127);
        }
        self.pid = Some(child_pid as u32);
        mem::drop(exec_fail_pipe.1);
        let mut error_string = String::new();
        exec_fail_pipe.0.read_to_string(&mut error_string).unwrap();
        if error_string.len() != 0 {
            let error_code: i32 = error_string.parse().unwrap();
            Err(Error::from_raw_os_error(error_code))
        } else {
            Ok(())
        }
    }
}
