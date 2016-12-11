use std::path::{PathBuf, Path};
use std::io::{Result, Error};

use libc::*;
use std::os::unix::ffi::OsStrExt;
use std::ptr;

#[derive(Debug)]
pub struct Popen {
    args: Vec<PathBuf>,
    pid: Option<pid_t>,
}

fn check_err<T: Ord + Default>(num: T) -> Result<T> {
    if num < T::default() {
        return Err(Error::last_os_error());
    }
    Ok(num)
}

fn path_as_ptr(p: &Path) -> *const c_char {
    let c_bytes = p.as_os_str().as_bytes();
    &c_bytes[0] as *const u8 as *const c_char
}

impl Popen {
    pub fn create<P: AsRef<Path>>(args: &[P]) -> Result<Popen> {
        let args: Vec<PathBuf> = args.iter()
            .map(|p| p.as_ref().to_owned()).collect();
        let mut inst = Popen {
            args: args,
            pid: None,
        };
        try!(inst.start());
        Ok(inst)
    }

    fn start(&mut self) -> Result<()> {
        let child_pid = try!(check_err(unsafe { fork() }));
        if child_pid == 0 {
            let cmd = &self.args[0];
            let mut args_os: Vec<_> = self.args.iter()
                .map(|x| path_as_ptr(x)).collect();
            args_os.push(ptr::null());
            let argv = &args_os[0] as *const *const c_char;
            check_err(unsafe { execvp(path_as_ptr(cmd), argv) }).unwrap();
        }
        self.pid = Some(child_pid);
        Ok(())
    }
}
