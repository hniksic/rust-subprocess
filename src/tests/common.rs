extern crate tempdir;
use self::tempdir::TempDir;

use std::fs::File;
use std::io::Read;
use std::io::Write;

use super::super::{Popen, PopenConfig, ExitStatus, Redirection, Popt};

pub fn read_whole_file(mut f: File) -> String {
    let mut content = String::new();
    f.read_to_string(&mut content).unwrap();
    content
}

#[test]
fn good_cmd() {
    let mut p = Popen::create(&["true"], PopenConfig::default()).unwrap();
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn bad_cmd() {
    let result = Popen::create(&["nosuchcommand"], PopenConfig::default());
    assert!(result.is_err());
}

#[test]
fn err_exit() {
    let mut p = Popen::create(&["sh", "-c", "exit 13"], PopenConfig::default())
        .unwrap();
    assert!(p.wait().unwrap() == ExitStatus::Exited(13));
}

#[test]
fn terminate() {
    let mut p = Popt::new("sleep").arg("1000").spawn().unwrap();
    p.terminate().unwrap();
    p.wait().unwrap();
}

#[test]
fn terminate_twice() {
    use std::thread;
    use std::time::Duration;

    let p = Popt::new("sleep").arg("1000").spawn().unwrap();
    p.terminate().unwrap();
    thread::sleep(Duration::from_millis(100));
    p.terminate().unwrap();
}

#[test]
fn read_from_stdout() {
    let mut p = Popen::create(&["echo", "foo"], PopenConfig {
        stdout: Redirection::Pipe, ..Default::default()
    }).unwrap();
    assert!(read_whole_file(p.stdout.take().unwrap()) == "foo\n");
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn input_from_file() {
    let tmpdir = TempDir::new("test").unwrap();
    let tmpname = tmpdir.path().join("input");
    {
        let mut outfile = File::create(&tmpname).unwrap();
        outfile.write_all(b"foo").unwrap();
    }
    let mut p = Popt::new("cat").arg(&tmpname)
        .stdin(Redirection::File(File::open(&tmpname).unwrap()))
        .stdout(Redirection::Pipe)
        .spawn()
        .unwrap();
    assert!(read_whole_file(p.stdout.take().unwrap()) == "foo");
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn output_to_file() {
    let tmpdir = TempDir::new("test").unwrap();
    let tmpname = tmpdir.path().join("output");
    let outfile = File::create(&tmpname).unwrap();
    let mut p = Popen::create(
        &["printf", "foo"], PopenConfig {
            stdout: Redirection::File(outfile), ..Default::default()
        }).unwrap();
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
    assert!(read_whole_file(File::open(&tmpname).unwrap()) == "foo");
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
    let mut p = Popen::create(
        &["cat"], PopenConfig {
            stdin: Redirection::File(File::open(&tmpname_in).unwrap()),
            stdout: Redirection::File(File::create(&tmpname_out).unwrap()),
            ..Default::default()
        }).unwrap();
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
    assert!(read_whole_file(File::open(&tmpname_out).unwrap()) == "foo");
}

#[test]
fn communicate_input() {
    let tmpdir = TempDir::new("test").unwrap();
    let tmpname = tmpdir.path().join("input");
    let mut p = Popen::create(
        &["cat"], PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::File(File::create(&tmpname).unwrap()),
            ..Default::default()
        }).unwrap();
    if let (None, None) = p.communicate_bytes(Some(b"hello world")).unwrap() {
    } else {
        assert!(false);
    }
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
    assert!(read_whole_file(File::open(&tmpname).unwrap()) == "hello world");
}

#[test]
fn communicate_output() {
    let mut p = Popen::create(
        &["sh", "-c", "echo foo; echo bar >&2"], PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        }).unwrap();
    if let (Some(out), Some(err)) = p.communicate_bytes(None).unwrap() {
        assert_eq!(out, b"foo\n");
        assert_eq!(err, b"bar\n");
    } else {
        assert!(false);
    }
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn communicate_input_output() {
    let mut p = Popen::create(
        &["sh", "-c", "cat; echo foo >&2"], PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        }).unwrap();
    if let (Some(out), Some(err)) = p.communicate_bytes(Some(b"hello world")).unwrap() {
        assert!(out == b"hello world");
        assert!(err == b"foo\n");
    } else {
        assert!(false);
    }
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn communicate_input_output_long() {
    let mut p = Popen::create(
        &["sh", "-c", "cat; printf '%100000s' '' >&2"], PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        }).unwrap();
    let input = [65u8; 1_000_000];
    if let (Some(out), Some(err)) = p.communicate_bytes(Some(&input)).unwrap() {
        assert!(&out[..] == &input[..]);
        assert!(&err[..] == &[32u8; 100_000][..]);
    } else {
        assert!(false);
    }
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn communicate_input_output_str() {
    let mut p = Popen::create(
        &["sh", "-c", "cat; echo foo >&2"], PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        }).unwrap();
    if let (Some(out), Some(err)) = p.communicate(Some("hello world")).unwrap() {
        assert!(out == "hello world");
        assert!(err == "foo\n");
    } else {
        assert!(false);
    }
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn null_byte_in_cmd() {
    let try_p = Popen::create(&["echo\0foo"], PopenConfig::default());
    assert!(try_p.is_err());
}

#[test]
fn merge_err_to_out_pipe() {
    let mut p = Popen::create(
        &["sh", "-c", "echo foo; echo bar >&2"], PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Merge,
            ..Default::default()
        }).unwrap();
    if let (Some(out), None) = p.communicate_bytes(None).unwrap() {
        assert_eq!(out, b"foo\nbar\n");
    } else {
        assert!(false);
    }
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn merge_out_to_err_pipe() {
    let mut p = Popen::create(
        &["sh", "-c", "echo foo; echo bar >&2"], PopenConfig {
            stdout: Redirection::Merge,
            stderr: Redirection::Pipe,
            ..Default::default()
        }).unwrap();
    if let (None, Some(err)) = p.communicate_bytes(None).unwrap() {
        assert_eq!(err, b"foo\nbar\n");
    } else {
        assert!(false);
    }
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn merge_err_to_out_file() {
    let tmpdir = TempDir::new("test").unwrap();
    let tmpname = tmpdir.path().join("output");
    let mut p = Popen::create(
        &["sh", "-c", "echo -n foo; echo -n bar >&2"], PopenConfig {
            stdout: Redirection::File(File::create(&tmpname).unwrap()),
            stderr: Redirection::Merge,
            ..Default::default()
        }).unwrap();
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
    assert!(read_whole_file(File::open(&tmpname).unwrap()) == "foobar");
}

#[test]
fn pipe_1() {
    let mut c1 = Popen::create(
        &["printf", "foo\\nbar\\nbaz\\n"], PopenConfig {
            stdout: Redirection::Pipe, ..Default::default()
        }).unwrap();
    let mut c2 = Popen::create(
        &["wc", "-l"], PopenConfig {
            stdin: Redirection::File(c1.stdout.take().unwrap()),
            stdout: Redirection::Pipe,
            ..Default::default()
        }).unwrap();
    let (wcout, _) = c2.communicate(None).unwrap();
    assert_eq!(wcout.unwrap().trim(), "3");
}
