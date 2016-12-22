extern crate libc;

#[cfg(windows)]
extern crate kernel32;
#[cfg(windows)]
extern crate winapi;

pub mod subprocess {
    mod popen;
    #[cfg(unix)]
    mod posix;
    #[cfg(windows)]
    mod win32;
    mod common;

    pub use self::common::ExitStatus;
    pub use self::popen::{Popen, Redirection};
}

#[cfg(all(test))]
mod tests_common {
    extern crate tempdir;
    use self::tempdir::TempDir;

    use std::path::Path;
    use std::fs::File;
    use std::io::Read;
    use std::io::Write;

    use subprocess::{Popen, ExitStatus, Redirection};

    pub fn read_whole_file(mut f: File) -> String {
        let mut content = String::new();
        f.read_to_string(&mut content).unwrap();
        content
    }

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
    fn read_from_stdout() {
        let mut p = Popen::create_full(
            &["echo", "foo"], Redirection::None, Redirection::Pipe, Redirection::None)
            .unwrap();
        assert!(read_whole_file(p.stdout.take().unwrap()) == "foo\n");
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
    }

    #[test]
    fn input_from_file() {
        let tmpdir = TempDir::new("test").unwrap();
        let tmpname = tmpdir.path().join("input");
        {
            let mut outfile = File::create(&tmpname).unwrap();
            outfile.write_all(b"foo").unwrap();
        }
        let mut p = Popen::create_full(
            &[Path::new("cat"), &tmpname],
            Redirection::File(File::open(&tmpname).unwrap()),
            Redirection::Pipe, Redirection::None)
            .unwrap();
        assert!(read_whole_file(p.stdout.take().unwrap()) == "foo");
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
    }
}

#[cfg(all(test, unix))]
mod tests_unix {
    extern crate tempdir;

    use subprocess::{Popen, ExitStatus, Redirection};
    use std::fs::File;
    use std::io::Write;
    use std::mem;
    use libc::SIGTERM;

    use self::tempdir::TempDir;

    use tests_common::read_whole_file;

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
        assert!(p.wait().unwrap() == Some(ExitStatus::Signaled(SIGTERM as u8)));
    }

    #[test]
    fn write_to_stdin() {
        let tmpdir = TempDir::new("test").unwrap();
        let tmpname = tmpdir.path().join("output");
        let mut p = Popen::create_full(
            &["dd".to_string(), format!("of={}", tmpname.display()), "status=none".to_string()],
            Redirection::Pipe, Redirection::None, Redirection::None)
            .unwrap();
        p.stdin.as_mut().unwrap().write_all(b"foo").unwrap();
        mem::drop(p.stdin.take());
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
        assert!(read_whole_file(File::open(tmpname).unwrap()) == "foo");
    }

    #[test]
    fn communicate_output() {
        let mut p = Popen::create_full(
            &["sh", "-c", "echo foo; echo bar >&2"],
            Redirection::None, Redirection::Pipe, Redirection::Pipe)
            .unwrap();
        if let (Some(out), Some(err)) = p.communicate_bytes(None).unwrap() {
            assert_eq!(out, b"foo\n");
            assert_eq!(err, b"bar\n");
        } else {
            assert!(false);
        }
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
    }

    #[test]
    fn communicate_input_output() {
        let mut p = Popen::create_full(
            &["sh", "-c", "cat; echo foo >&2"],
            Redirection::Pipe, Redirection::Pipe, Redirection::Pipe)
            .unwrap();
        if let (Some(out), Some(err)) = p.communicate_bytes(Some(b"hello world")).unwrap() {
            assert!(out == b"hello world");
            assert!(err == b"foo\n");
        } else {
            assert!(false);
        }
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
    }

    #[test]
    fn communicate_input_output_long() {
        let mut p = Popen::create_full(
            &["sh", "-c", "cat; printf '%100000s' '' >&2"],
            Redirection::Pipe, Redirection::Pipe, Redirection::Pipe)
            .unwrap();
        let input = [65u8; 1_000_000];
        if let (Some(out), Some(err)) = p.communicate_bytes(Some(&input)).unwrap() {
            assert!(&out[..] == &input[..]);
            assert!(&err[..] == &[32u8; 100_000][..]);
        } else {
            assert!(false);
        }
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
    }

    #[test]
    fn communicate_input_output_str() {
        let mut p = Popen::create_full(
            &["sh", "-c", "cat; echo foo >&2"],
            Redirection::Pipe, Redirection::Pipe, Redirection::Pipe)
            .unwrap();
        if let (Some(out), Some(err)) = p.communicate(Some("hello world")).unwrap() {
            assert!(out == "hello world");
            assert!(err == "foo\n");
        } else {
            assert!(false);
        }
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
    }

    #[test]
    fn output_to_file() {
        let tmpdir = TempDir::new("test").unwrap();
        let tmpname = tmpdir.path().join("output");
        let outfile = File::create(&tmpname).unwrap();
        let mut p = Popen::create_full(
            &["echo", "foo"], Redirection::None, Redirection::File(outfile), Redirection::None)
            .unwrap();
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
        assert!(read_whole_file(File::open(&tmpname).unwrap()) == "foo\n");
    }

    #[test]
    fn communicate_input() {
        let tmpdir = TempDir::new("test").unwrap();
        let tmpname = tmpdir.path().join("input");
        let mut p = Popen::create_full(
            &["cat"],
            Redirection::Pipe,
            Redirection::File(File::create(&tmpname).unwrap()),
            Redirection::None)
            .unwrap();
        if let (None, None) = p.communicate_bytes(Some(b"hello world")).unwrap() {
        } else {
            assert!(false);
        }
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
        assert!(read_whole_file(File::open(&tmpname).unwrap()) == "hello world");
    }

    #[test]
    fn input_output_from_file() {
        let tmpdir = TempDir::new("test").unwrap();
        let tmpname_in = tmpdir.path().join("input");
        let tmpname_out = tmpdir.path().join("output");
        {
            let mut f = File::create(&tmpname_in).unwrap();
            f.write_all(b"foo").unwrap();
        }
        let mut p = Popen::create_full(
            &["cat"],
            Redirection::File(File::open(&tmpname_in).unwrap()),
            Redirection::File(File::create(&tmpname_out).unwrap()),
            Redirection::None)
            .unwrap();
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(0)));
        assert!(read_whole_file(File::open(&tmpname_out).unwrap()) == "foo");
    }
}

#[cfg(all(test, windows))]
mod tests_win32 {
    use subprocess::{Popen, ExitStatus};

    #[test]
    fn non_zero_exit() {
        let mut p = Popen::create(&["bash", "-c", "\"exit 13\""]).unwrap();
        assert!(p.wait().unwrap() == Some(ExitStatus::Exited(13)));
    }
}
