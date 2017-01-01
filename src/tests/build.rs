extern crate tempdir;

use std::fs::File;

use super::super::{Run, Redirection, NullFile};

use self::tempdir::TempDir;

use tests::common::read_whole_file;

#[test]
fn null_file() {
    let mut p = Run::cmd("cat")
        .stdin(NullFile).stdout(Redirection::Pipe)
        .popen().unwrap();
    let (out, _) = p.communicate(None).unwrap();
    assert!(out.unwrap() == "");
}

#[test]
fn stream_stdout() {
    let stream = Run::cmd("echo")
        .args(&["-n", "foo"])
        .stream_stdout().unwrap();
    assert!(read_whole_file(stream) == "foo");
}

#[test]
fn stream_stderr() {
    let stream = Run::cmd("sh")
        .args(&["-c", "echo -n foo >&2"])
        .stream_stderr().unwrap();
    assert!(read_whole_file(stream) == "foo");
}

#[test]
fn stream_stdin() {
    let tmpdir = TempDir::new("test").unwrap();
    let tmpname = tmpdir.path().join("output");
    {
        let mut stream = Run::cmd("cat")
            .stdout(File::create(&tmpname).unwrap())
            .stream_stdin().unwrap();
        stream.write_all(b"foo").unwrap();
    }
    assert_eq!(read_whole_file(File::open(&tmpname).unwrap()), "foo");
}

#[test]
fn shell_exec() {
    // note: this uses built-in echo on Windows, so don't try anything
    // fancy like echo -n
    let stream = Run::shell("echo foo").stream_stdout().unwrap();
    assert_eq!(read_whole_file(stream).trim(), "foo");
}

#[test]
fn pipeline_run() {
    let mut processes = {
        Run::cmd("echo").arg("foo\nbar") | Run::cmd("wc").arg("-l")
    }
    .stdout(Redirection::Pipe).popen().unwrap();
    let (output, _) = processes[1].communicate(None).unwrap();
    assert!(output.unwrap().trim() == "2");
}

#[test]
fn pipeline_stream_out() {
    let stream = {
        Run::cmd("echo").arg("foo\nbar") | Run::cmd("wc").arg("-l")
    }.stream_stdout().unwrap();
    assert!(read_whole_file(stream).trim() == "2");
}

#[test]
fn pipeline_stream_in() {
    let tmpdir = TempDir::new("test").unwrap();
    let tmpname = tmpdir.path().join("output");
    {
        let mut stream = {
            Run::cmd("cat")
          | Run::cmd("wc").arg("-l")
        }.stdout(File::create(&tmpname).unwrap())
         .stream_stdin().unwrap();
        stream.write_all(b"foo\nbar\nbaz\n").unwrap();
    }
    assert_eq!(read_whole_file(File::open(&tmpname).unwrap()).trim(), "3");
}
