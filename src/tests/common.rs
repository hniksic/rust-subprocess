extern crate tempdir;
use self::tempdir::TempDir;

use std::path::Path;
use std::fs::File;
use std::io::Read;
use std::io::Write;

use super::super::{Popen, ExitStatus, Redirection};

pub fn read_whole_file(mut f: File) -> String {
    let mut content = String::new();
    f.read_to_string(&mut content).unwrap();
    content
}

#[test]
fn good_cmd() {
    let mut p = Popen::create(&["true"]).unwrap();
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
fn read_from_stdout() {
    let mut p = Popen::create_full(
        &["echo", "foo"], Redirection::None, Redirection::Pipe, Redirection::None)
        .unwrap();
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
    let mut p = Popen::create_full(
        &[Path::new("cat"), &tmpname],
        Redirection::File(File::open(&tmpname).unwrap()),
        Redirection::Pipe, Redirection::None)
        .unwrap();
    assert!(read_whole_file(p.stdout.take().unwrap()) == "foo");
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn output_to_file() {
    let tmpdir = TempDir::new("test").unwrap();
    let tmpname = tmpdir.path().join("output");
    let outfile = File::create(&tmpname).unwrap();
    let mut p = Popen::create_full(
        &["printf", "foo"],
        Redirection::None, Redirection::File(outfile), Redirection::None)
        .unwrap();
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
    let mut p = Popen::create_full(
        &["cat"],
        Redirection::File(File::open(&tmpname_in).unwrap()),
        Redirection::File(File::create(&tmpname_out).unwrap()),
        Redirection::None)
        .unwrap();
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
    assert!(read_whole_file(File::open(&tmpname_out).unwrap()) == "foo");
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
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
    assert!(read_whole_file(File::open(&tmpname).unwrap()) == "hello world");
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
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
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
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
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
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
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
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn null_byte_in_cmd() {
    let try_p = Popen::create_full(
        &["echo\0foo"], Redirection::None, Redirection::None, Redirection::None);
    assert!(try_p.is_err());
}

#[test]
fn err_to_out() {
    let mut p = Popen::create_full(
        &["sh", "-c", "echo foo; echo bar >&2"],
        Redirection::None, Redirection::Pipe, Redirection::Merge)
        .unwrap();
    if let (Some(out), None) = p.communicate_bytes(None).unwrap() {
        assert_eq!(out, b"foo\nbar\n");
    } else {
        assert!(false);
    }
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn out_to_err() {
    let mut p = Popen::create_full(
        &["sh", "-c", "echo foo; echo bar >&2"],
        Redirection::None, Redirection::Merge, Redirection::Pipe)
        .unwrap();
    if let (None, Some(err)) = p.communicate_bytes(None).unwrap() {
        assert_eq!(err, b"foo\nbar\n");
    } else {
        assert!(false);
    }
    assert!(p.wait().unwrap() == ExitStatus::Exited(0));
}

#[test]
fn out_to_err_no_redirection() {
    {
        let mut p = Popen::create_full(
            &["sh", "-c", "echo foo; echo bar >&2"],
            Redirection::None, Redirection::Merge, Redirection::None)
            .unwrap();
        assert!(p.wait().unwrap() == ExitStatus::Exited(0));
    }
    {
        let mut p = Popen::create_full(
            &["sh", "-c", "echo foo; echo bar >&2"],
            Redirection::None, Redirection::None, Redirection::Merge)
            .unwrap();
        assert!(p.wait().unwrap() == ExitStatus::Exited(0));
    }
}
