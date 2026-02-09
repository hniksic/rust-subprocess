use std::fs::{self, File};
use std::io::{self, ErrorKind, prelude::*};
use std::time::Duration;

use tempfile::TempDir;

use crate::{Exec, Pipeline, Redirection};

#[test]
fn simple_pipe() {
    let c = (Exec::cmd("printf").arg("foo\\nbar\\nbaz\\n") | Exec::cmd("wc").arg("-l"))
        .capture()
        .unwrap();
    assert_eq!(c.stdout_str().trim(), "3");
}

#[test]
fn pipeline_stream_out() {
    let stream = { Exec::cmd("echo").arg("foo\nbar") | Exec::cmd("wc").arg("-l") }
        .stream_stdout()
        .unwrap();
    assert_eq!(io::read_to_string(stream).unwrap().trim(), "2");
}

#[test]
fn pipeline_stream_in() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("output");
    {
        let mut stream = { Exec::cmd("cat") | Exec::cmd("wc").arg("-l") }
            .stdout(File::create(&tmpname).unwrap())
            .stream_stdin()
            .unwrap();
        stream.write_all(b"foo\nbar\nbaz\n").unwrap();
    }
    assert_eq!(fs::read_to_string(&tmpname).unwrap().trim(), "3");
}

#[test]
fn pipeline_stream_err() {
    let stream = { Exec::cmd("sh").args(&["-c", "printf foo >&2"]) | Exec::cmd("true") }
        .stream_stderr_all()
        .unwrap();
    assert_eq!(io::read_to_string(stream).unwrap(), "foo");
}

#[test]
fn pipeline_compose_pipelines() {
    let pipe1 = Exec::cmd("echo").arg("foo\nbar\nfoo") | Exec::cmd("sort");
    let pipe2 = Exec::cmd("uniq") | Exec::cmd("wc").arg("-l");
    let pipe = pipe1 | pipe2;
    let stream = pipe.stream_stdout().unwrap();
    assert_eq!(io::read_to_string(stream).unwrap().trim(), "2");
}

trait Crlf {
    fn to_crlf(self) -> Vec<u8>;
}
impl Crlf for Vec<u8> {
    #[cfg(windows)]
    fn to_crlf(self) -> Vec<u8> {
        self.iter()
            .flat_map(|&c| {
                if c == b'\n' {
                    vec![b'\r', b'\n']
                } else {
                    vec![c]
                }
            })
            .collect()
    }
    #[cfg(unix)]
    fn to_crlf(self) -> Vec<u8> {
        self
    }
}

#[test]
fn pipeline_communicate_out() {
    let pipe1 = Exec::cmd("echo").arg("foo\nbar\nfoo") | Exec::cmd("sort");
    let mut comm = pipe1.communicate().unwrap();
    assert_eq!(
        comm.read().unwrap(),
        (b"bar\nfoo\nfoo\n".to_vec().to_crlf(), vec![])
    );
}

#[test]
fn pipeline_communicate_in_out() {
    let pipe1 = Exec::cmd("grep").arg("foo") | Exec::cmd("sort");
    let mut comm = pipe1.stdin("foobar\nbaz\nfoo\n").communicate().unwrap();
    let (out, _err) = comm.read().unwrap();
    assert_eq!(out, b"foo\nfoobar\n".to_vec().to_crlf());
}

#[test]
fn pipeline_capture() {
    let c = { Exec::cmd("cat") | Exec::shell("wc -l") }
        .stdin("foo\nbar\nbaz\n")
        .capture()
        .unwrap();
    assert_eq!(c.stdout_str().trim(), "3");
    assert_eq!(c.stderr_str().trim(), "");
}

#[test]
fn pipeline_capture_error_1() {
    let c = {
        Exec::cmd("sh")
            .arg("-c")
            .arg("echo foo >&2; printf 'bar\nbaz\n'")
            | Exec::shell("wc -l")
    }
    .capture()
    .unwrap();
    assert_eq!(c.stdout_str().trim(), "2");
    assert_eq!(c.stderr_str().trim(), "foo");
}

#[test]
fn pipeline_capture_error_2() {
    let c = {
        Exec::cmd("cat")
            | Exec::cmd("sh")
                .arg("-c")
                .arg("cat; echo foo >&2; printf 'four\nfive\n'")
            | Exec::cmd("sh").arg("-c").arg("echo bar >&2; cat")
            | Exec::shell("wc -l")
    }
    .stdin("one\ntwo\nthree\n")
    .capture()
    .unwrap();
    assert_eq!(c.stdout_str().trim(), "5");
    assert!(
        c.stderr_str().trim() == "foo\nbar" || c.stderr_str().trim() == "bar\nfoo",
        "got {:?}",
        c.stderr_str()
    );
}

#[test]
fn pipeline_join() {
    let status = (Exec::cmd("true") | Exec::cmd("true")).join().unwrap();
    assert!(status.success());

    let status = (Exec::cmd("false") | Exec::cmd("true")).join().unwrap();
    assert!(status.success());

    let status = (Exec::cmd("true") | Exec::cmd("false")).join().unwrap();
    assert_eq!(status.code(), Some(1));
}

#[test]
fn pipeline_invalid_1() {
    let p = (Exec::cmd("echo").arg("foo") | Exec::cmd("no-such-command")).join();
    assert!(p.is_err());
}

#[test]
fn pipeline_invalid_2() {
    let p = (Exec::cmd("no-such-command") | Exec::cmd("echo").arg("foo")).join();
    assert!(p.is_err());
}

#[test]
fn pipeline_rejects_first_cmd_stdin() {
    let first = Exec::cmd("cat").stdin(Redirection::Pipe);
    let err = (first | Exec::cmd("wc")).start().unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert!(err.to_string().contains("stdin of the first command"));
}

#[test]
fn pipeline_rejects_last_cmd_stdout() {
    let last = Exec::cmd("wc").stdout(Redirection::Null);
    let err = (Exec::cmd("echo") | last).start().unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert!(err.to_string().contains("stdout of the last command"));
}

#[test]
fn pipeline_to_string() {
    let pipeline = { Exec::cmd("command with space").arg("arg") | Exec::cmd("wc").arg("-l") };
    assert_eq!(
        format!("{:?}", pipeline),
        "Pipeline { 'command with space' arg | wc -l }"
    )
}

#[test]
fn pipeline_stderr_all_merge() {
    // stderr_all(Merge) redirects each command's stderr to its stdout,
    // so stderr output flows through the pipeline into captured stdout.
    let c = { Exec::cmd("sh").arg("-c").arg("echo from-stderr >&2") | Exec::cmd("cat") }
        .stderr_all(Redirection::Merge)
        .capture()
        .unwrap();
    assert!(
        c.stdout_str().contains("from-stderr"),
        "stdout should contain stderr output, got: {:?}",
        c.stdout_str()
    );
    assert_eq!(c.stderr_str(), "");
}

#[test]
fn pipeline_stderr_all_file() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("stderr_output");
    {
        let f = File::create(&tmpname).unwrap();
        let _status = {
            Exec::cmd("sh").arg("-c").arg("echo err1 >&2; echo out1")
                | Exec::cmd("sh").arg("-c").arg("cat; echo err2 >&2")
        }
        .stderr_all(f)
        .join()
        .unwrap();
    }
    let stderr_content = fs::read_to_string(&tmpname).unwrap();
    assert!(
        stderr_content.contains("err1"),
        "file should contain err1, got: {:?}",
        stderr_content
    );
    assert!(
        stderr_content.contains("err2"),
        "file should contain err2, got: {:?}",
        stderr_content
    );
}

#[test]
fn pipeline_stderr_all_pipe_capture() {
    // Explicitly requesting Pipe should work with capture(), capturing
    // stderr from all commands.
    let c = {
        Exec::cmd("sh").arg("-c").arg("echo err1 >&2; echo out1")
            | Exec::cmd("sh").arg("-c").arg("cat; echo err2 >&2")
    }
    .stderr_all(Redirection::Pipe)
    .capture()
    .unwrap();
    assert!(
        c.stderr_str().contains("err1"),
        "stderr should contain err1, got: {:?}",
        c.stderr_str()
    );
    assert!(
        c.stderr_str().contains("err2"),
        "stderr should contain err2, got: {:?}",
        c.stderr_str()
    );
}

#[test]
fn pipeline_capture_preserves_stderr_merge() {
    // Pipeline::capture() auto-sets stdout and stderr independently.
    // stderr_all(Merge) is not None, so auto-stderr is skipped.
    // stdout is None, so it IS auto-piped.
    // Result: both stdout and stderr content end up in captured stdout.
    let c = { Exec::cmd("sh").arg("-c").arg("echo err >&2; echo out") | Exec::cmd("cat") }
        .stderr_all(Redirection::Merge)
        .capture()
        .unwrap();
    assert!(
        c.stdout_str().contains("out"),
        "stdout should contain 'out', got: {:?}",
        c.stdout_str()
    );
    assert!(
        c.stdout_str().contains("err"),
        "stdout should contain 'err' (merged), got: {:?}",
        c.stdout_str()
    );
    assert_eq!(c.stderr_str(), "");
}

#[test]
fn pipeline_communicate_auto_pipes() {
    // Pipeline::communicate() auto-sets both stdout and stderr Pipe
    // independently (when they are None).
    let mut comm = { Exec::cmd("echo").arg("foo") | Exec::cmd("cat") }
        .communicate()
        .unwrap();
    let (stdout, _stderr) = comm.read().unwrap();
    assert_eq!(String::from_utf8_lossy(&stdout).trim(), "foo");
}

#[test]
fn pipeline_cwd() {
    let tmpdir = TempDir::new().unwrap();
    (Exec::cmd("touch").arg("here") | Exec::cmd("cat"))
        .cwd(tmpdir.path())
        .capture()
        .unwrap();
    assert!(tmpdir.path().join("here").exists());
}

#[test]
fn pipeline_cwd_from_iter() {
    let tmpdir = TempDir::new().unwrap();
    let pipeline: Pipeline = vec![Exec::cmd("touch").arg("here"), Exec::cmd("cat")]
        .into_iter()
        .collect();
    pipeline.cwd(tmpdir.path()).capture().unwrap();
    assert!(tmpdir.path().join("here").exists());
}

#[test]
fn pipeline_empty_capture() {
    let c = Pipeline::new().capture().unwrap();
    assert!(c.success());
    assert_eq!(c.stdout_str(), "");
    assert_eq!(c.stderr_str(), "");
}

#[test]
fn pipeline_empty_join() {
    let status = Pipeline::new().join().unwrap();
    assert!(status.success());
}

#[test]
fn pipeline_single_command_capture() {
    let c = Pipeline::new()
        .pipe(Exec::cmd("printf").arg("hello"))
        .capture()
        .unwrap();
    assert!(c.success());
    assert_eq!(c.stdout_str(), "hello");
}

#[test]
fn pipeline_single_command_join() {
    let status = Pipeline::new().pipe(Exec::cmd("true")).join().unwrap();
    assert!(status.success());

    let status = Pipeline::new().pipe(Exec::cmd("false")).join().unwrap();
    assert!(!status.success());
}

#[test]
fn pipeline_builder_two_commands() {
    let c = Pipeline::new()
        .pipe(Exec::cmd("echo").arg("foo\nbar"))
        .pipe(Exec::cmd("wc").arg("-l"))
        .capture()
        .unwrap();
    assert_eq!(c.stdout_str().trim(), "2");
}

#[test]
fn pipeline_from_iter_empty() {
    let pipeline: Pipeline = vec![].into_iter().collect();
    let c = pipeline.capture().unwrap();
    assert!(c.success());
    assert_eq!(c.stdout_str(), "");
}

#[test]
fn pipeline_from_iter_single() {
    let pipeline: Pipeline = vec![Exec::cmd("printf").arg("hi")].into_iter().collect();
    let c = pipeline.capture().unwrap();
    assert_eq!(c.stdout_str(), "hi");
}

#[test]
fn pipeline_single_command_with_stdin_data() {
    let c = Pipeline::new()
        .pipe(Exec::cmd("cat"))
        .stdin("hello")
        .capture()
        .unwrap();
    assert_eq!(c.stdout_str(), "hello");
}

#[test]
fn pipeline_default() {
    let c = Pipeline::default().capture().unwrap();
    assert!(c.success());
}

#[test]
fn pipeline_empty_start() {
    let job = Pipeline::new().start().unwrap();
    assert!(job.processes.is_empty());
    assert!(job.stdin.is_none());
    assert!(job.stdout.is_none());
    assert!(job.stderr.is_none());
}

#[test]
fn pipeline_empty_pids() {
    let job = Pipeline::new().start().unwrap();
    assert!(job.pids().is_empty());
}

#[test]
#[should_panic]
fn pipeline_empty_pid_panics() {
    let job = Pipeline::new().start().unwrap();
    job.pid();
}

#[test]
fn pipeline_empty_wait() {
    let job = Pipeline::new().start().unwrap();
    let status = job.wait().unwrap();
    assert!(status.success());
}

#[test]
fn pipeline_empty_wait_timeout() {
    let job = Pipeline::new().start().unwrap();
    let status = job.wait_timeout(Duration::from_secs(1)).unwrap();
    assert!(status.unwrap().success());
}

#[test]
fn pipeline_empty_poll() {
    let job = Pipeline::new().start().unwrap();
    assert!(job.poll().unwrap().success());
}

#[test]
fn pipeline_empty_terminate() {
    let job = Pipeline::new().start().unwrap();
    job.terminate().unwrap();
}

#[test]
fn pipeline_empty_kill() {
    let job = Pipeline::new().start().unwrap();
    job.kill().unwrap();
}

#[test]
fn pipeline_empty_detach() {
    let job = Pipeline::new().start().unwrap();
    job.detach();
}

#[test]
fn pipeline_empty_job_join() {
    let job = Pipeline::new().start().unwrap();
    let status = job.join().unwrap();
    assert!(status.success());
}

#[test]
fn pipeline_empty_job_join_timeout() {
    let job = Pipeline::new().start().unwrap();
    let status = job.join_timeout(Duration::from_secs(1)).unwrap();
    assert!(status.success());
}

#[test]
fn pipeline_empty_job_capture() {
    let job = Pipeline::new().start().unwrap();
    let c = job.capture().unwrap();
    assert!(c.success());
    assert_eq!(c.stdout_str(), "");
    assert_eq!(c.stderr_str(), "");
}

#[test]
fn pipeline_empty_job_capture_timeout() {
    let job = Pipeline::new().start().unwrap();
    let c = job.capture_timeout(Duration::from_secs(1)).unwrap();
    assert!(c.success());
    assert_eq!(c.stdout_str(), "");
    assert_eq!(c.stderr_str(), "");
}

#[test]
fn pipeline_empty_communicate() {
    let mut comm = Pipeline::new().communicate().unwrap();
    let (stdout, stderr) = comm.read().unwrap();
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn pipeline_empty_checked_join() {
    // Empty pipeline returns success, so checked doesn't trigger an error.
    Pipeline::new().checked().join().unwrap();
}

#[test]
fn pipeline_empty_checked_capture() {
    let c = Pipeline::new().checked().capture().unwrap();
    assert!(c.success());
    assert_eq!(c.stdout_str(), "");
}

#[test]
fn pipeline_checked() {
    // Last command fails -> error
    let err = (Exec::cmd("true") | Exec::cmd("false"))
        .checked()
        .join()
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Other);
    assert!(err.to_string().contains("command failed"), "{err}");

    // Last command succeeds -> ok
    (Exec::cmd("false") | Exec::cmd("true"))
        .checked()
        .join()
        .unwrap();
}
