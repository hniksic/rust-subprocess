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

    #[test]
    fn output_to_file() {
        let tmpname = "/tmp/bar";
        let outfile = File::create(tmpname).unwrap();
        let mut p = Popen::create_full(
            &["echo", "foo"], Redirection::None, Redirection::File(outfile), Redirection::None)
            .unwrap();
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
        let mut file_contents = String::new();
        File::open(tmpname).unwrap().read_to_string(&mut file_contents).unwrap();
        assert!(file_contents == "foo\n");
    }

    #[test]
    fn input_from_file() {
        let tmpname = "/tmp/baz";
        {
            let mut outfile = File::create(tmpname).unwrap();
            outfile.write_all(b"foo").unwrap();
        }
        let mut p = Popen::create_full(
            &["cat", tmpname],
            Redirection::File(File::open(tmpname).unwrap()),
            Redirection::Pipe,
            Redirection::None)
            .unwrap();
        let mut output = String::new();
        p.stdout.as_mut().unwrap().read_to_string(&mut output).unwrap();
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
        assert!(output == "foo");
    }

    #[test]
    fn input_output_from_file() {
        let tmpname_in = "/tmp/qux-in";
        let tmpname_out = "/tmp/qux-out";
        {
            let mut f = File::create(tmpname_in).unwrap();
            f.write_all(b"foo").unwrap();
        }
        let mut p = Popen::create_full(
            &["cat"],
            Redirection::File(File::open(tmpname_in).unwrap()),
            Redirection::File(File::create(tmpname_out).unwrap()),
            Redirection::None)
            .unwrap();
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
        let mut output = String::new();
        File::open(tmpname_out).unwrap().read_to_string(&mut output).unwrap();
        assert!(output == "foo");
    }
}
