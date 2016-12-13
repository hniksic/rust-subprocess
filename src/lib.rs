extern crate libc;

mod posix;
pub mod popen;

#[cfg(test)]
mod tests {
    use popen;
    use popen::{Popen, ExitStatus, Redirection};
    use std::fs::File;
    use std::io::{Read, Write};
    use std::mem;

    #[test]
    fn good_cmd() {
        let mut p = Popen::create(&["true"]).unwrap();
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
    }

    #[test]
    fn bad_cmd() {
        let result = Popen::create(&["nosuchcommand"]);
        assert!(result.is_err());
    }

    #[test]
    fn err_exit() {
        let mut p = Popen::create(&["sh", "-c", "exit 13"]).unwrap();
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(13)));
    }

    #[test]
    fn err_signal() {
        let mut p = Popen::create(&["sleep", "5"]).unwrap();
        assert!(p.poll().is_none());
        p.terminate().unwrap();
        assert!(p.wait().unwrap() == Some(ExitStatus::Signaled(popen::SIGTERM)));
    }

    #[test]
    fn read_from_stdout() {
        let mut p = Popen::create_full(
            &["echo", "foo"], Redirection::None, Redirection::Pipe, Redirection::None)
            .unwrap();
        let mut output = String::new();
        p.stdout.as_mut().unwrap().read_to_string(&mut output).unwrap();
        assert!(output == "foo\n");
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
    }

    #[test]
    fn write_to_stdin() {
        let tmpname = "/tmp/foo";
        let mut p = Popen::create_full(
            &["dd".to_string(), format!("of={}", tmpname), "status=none".to_string()],
            Redirection::Pipe, Redirection::None, Redirection::None)
            .unwrap();
        p.stdin.as_mut().unwrap().write_all(b"foo").unwrap();
        mem::drop(p.stdin.take());
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
        let mut file_contents = String::new();
        File::open(tmpname).unwrap().read_to_string(&mut file_contents).unwrap();
        assert!(file_contents == "foo");
    }
}
