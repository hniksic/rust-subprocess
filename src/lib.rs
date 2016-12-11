extern crate libc;

pub mod popen;

#[cfg(test)]
mod tests {
    use popen;
    use popen::{Popen, ExitStatus};

    #[test]
    fn good_cmd() {
        let mut p = Popen::create(&["ls", "-al"]).unwrap();
        assert!(p.wait().unwrap() == ExitStatus::Exited(0));
    }

    #[test]
    fn bad_cmd() {
        let result = Popen::create(&["nosuchcommand"]);
        assert!(result.is_err());
    }

    #[test]
    fn err_exit() {
        let mut p = Popen::create(&["sh", "-c", "exit 13"]).unwrap();
        assert!(p.wait().unwrap() == ExitStatus::Exited(13));
    }

    #[test]
    fn err_signal() {
        let mut p = Popen::create(&["sleep", "5"]).unwrap();
        p.terminate().unwrap();
        assert!(p.wait().unwrap() == ExitStatus::Signaled(popen::SIGTERM));
    }
}
