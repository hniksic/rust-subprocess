use std::env;
use std::fs::{self, File};
use std::io::{self, ErrorKind, prelude::*};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tempfile::TempDir;

use crate::{Exec, Redirection};

#[test]
fn exec_join() {
    let status = Exec::cmd("true").join().unwrap();
    assert!(status.success());
}

#[test]
fn null_file() {
    let mut p = Exec::cmd("cat")
        .stdin(Redirection::Null)
        .stdout(Redirection::Pipe)
        .popen()
        .unwrap();
    let (out, _) = p.communicate([]).unwrap().read_string().unwrap();
    assert_eq!(out, "");
}

#[test]
fn stream_stdout() {
    let stream = Exec::cmd("printf").arg("foo").stream_stdout().unwrap();
    assert_eq!(io::read_to_string(stream).unwrap(), "foo");
}

#[test]
fn stream_stderr() {
    let stream = Exec::cmd("sh")
        .args(&["-c", "printf foo >&2"])
        .stream_stderr()
        .unwrap();
    assert_eq!(io::read_to_string(stream).unwrap(), "foo");
}

#[test]
fn stream_stdin() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("output");
    {
        let mut stream = Exec::cmd("cat")
            .stdout(File::create(&tmpname).unwrap())
            .stream_stdin()
            .unwrap();
        stream.write_all(b"foo").unwrap();
    }
    assert_eq!(fs::read_to_string(&tmpname).unwrap(), "foo");
}

#[test]
fn communicate_out() {
    let mut comm = Exec::cmd("printf").arg("foo").communicate().unwrap();
    assert_eq!(comm.read().unwrap(), (b"foo".to_vec(), vec![]));
}

#[test]
fn communicate_in_out() {
    let mut comm = Exec::cmd("cat").stdin("foo").communicate().unwrap();
    assert_eq!(comm.read().unwrap(), (b"foo".to_vec(), vec![]));
}

#[test]
fn capture_out() {
    let c = Exec::cmd("printf").arg("foo").capture().unwrap();
    assert_eq!(c.stdout_str(), "foo");
}

#[test]
fn capture_err() {
    let c = Exec::cmd("sh")
        .arg("-c")
        .arg("printf foo >&2")
        .stderr(Redirection::Pipe)
        .capture()
        .unwrap();
    assert_eq!(c.stderr_str(), "foo");
}

#[test]
fn capture_out_with_input_data1() {
    let c = Exec::cmd("cat").stdin("foo").capture().unwrap();
    assert_eq!(c.stdout_str(), "foo");
}

#[test]
fn capture_out_with_input_data2() {
    let c = Exec::cmd("cat").stdin(b"foo".to_vec()).capture().unwrap();
    assert_eq!(c.stdout_str(), "foo");
}

#[test]
fn exec_shell() {
    let stream = Exec::shell("printf foo").stream_stdout().unwrap();
    assert_eq!(io::read_to_string(stream).unwrap(), "foo");
}

#[test]
fn pipeline_open() {
    let mut processes = { Exec::cmd("echo").arg("foo\nbar") | Exec::cmd("wc").arg("-l") }
        .stdout(Redirection::Pipe)
        .popen()
        .unwrap();
    let (output, _) = processes[1].communicate([]).unwrap().read_string().unwrap();
    assert_eq!(output.trim(), "2");
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
#[should_panic(expected = "stdin of the first command is already redirected")]
fn pipeline_rejects_first_cmd_stdin() {
    let first = Exec::cmd("cat").stdin(Redirection::Pipe);
    let _pipeline = first | Exec::cmd("wc");
}

#[test]
#[should_panic(expected = "stdout of the last command is already redirected")]
fn pipeline_rejects_last_cmd_stdout() {
    let last = Exec::cmd("wc").stdout(Redirection::Null);
    let _pipeline = Exec::cmd("echo") | last;
}

#[test]
#[should_panic]
fn reject_input_data_popen() {
    Exec::cmd("true").stdin("xxx").popen().unwrap();
}

#[test]
#[should_panic]
fn reject_input_data_join() {
    Exec::cmd("true").stdin("xxx").join().unwrap();
}

#[test]
#[should_panic]
fn reject_input_data_stream_stdout() {
    Exec::cmd("true").stdin("xxx").stream_stdout().unwrap();
}

#[test]
#[should_panic]
fn reject_input_data_stream_stderr() {
    Exec::cmd("true").stdin("xxx").stream_stderr().unwrap();
}

#[test]
#[should_panic]
fn reject_input_data_stream_stdin() {
    Exec::cmd("true").stdin("xxx").stream_stdin().unwrap();
}

#[test]
fn env_set() {
    assert!(
        Exec::cmd("sh")
            .args(&["-c", r#"test "$SOMEVAR" = "foo""#])
            .env("SOMEVAR", "foo")
            .join()
            .unwrap()
            .success()
    );
}

#[test]
fn env_extend() {
    assert!(
        Exec::cmd("sh")
            .args(&["-c", r#"test "$VAR1" = "foo" && test "$VAR2" = "bar""#])
            .env_extend([("VAR1", "foo"), ("VAR2", "bar")])
            .join()
            .unwrap()
            .success()
    );
}

static MUTATE_ENV: Mutex<()> = Mutex::new(());

struct TmpEnvVar<'a> {
    varname: &'static str,
    #[allow(dead_code)]
    mutate_guard: MutexGuard<'a, ()>,
}

impl<'a> TmpEnvVar<'a> {
    fn new(varname: &'static str) -> TmpEnvVar<'a> {
        TmpEnvVar {
            varname,
            mutate_guard: MUTATE_ENV.lock().unwrap(),
        }
    }
}

impl Drop for TmpEnvVar<'_> {
    fn drop(&mut self) {
        // SAFETY: We hold a mutex guard that serializes all env var modifications in
        // tests
        unsafe { env::remove_var(self.varname) };
    }
}

fn tmp_env_var<'a>(varname: &'static str, tmp_value: &'static str) -> TmpEnvVar<'a> {
    // Acquire the lock first to avoid race with other tests that modify env vars
    let guard = TmpEnvVar::new(varname);
    // SAFETY: We hold a mutex guard that serializes all env var modifications in tests
    unsafe { env::set_var(varname, tmp_value) };
    guard
}

#[test]
fn env_inherit() {
    // use a unique name to avoid interference with other tests
    let varname = "TEST_ENV_INHERIT_VARNAME";
    let _guard = tmp_env_var(varname, "inherited");
    assert!(
        Exec::cmd("sh")
            .args(&["-c", &format!(r#"test "${}" = "inherited""#, varname)])
            .join()
            .unwrap()
            .success()
    );
}

#[test]
fn env_inherit_set() {
    // use a unique name to avoid interference with other tests
    let varname = "TEST_ENV_INHERIT_SET_VARNAME";
    let _guard = tmp_env_var(varname, "inherited");
    assert!(
        Exec::cmd("sh")
            .args(&["-c", &format!(r#"test "${}" = "new""#, varname)])
            .env(varname, "new")
            .join()
            .unwrap()
            .success()
    );
}

#[test]
fn exec_to_string() {
    let _guard = MUTATE_ENV.lock().unwrap();
    let cmd = Exec::cmd("sh")
        .arg("arg1")
        .arg("don't")
        .arg("arg3 arg4")
        .arg("?")
        .arg(" ") // regular space
        .arg("\u{009c}"); // STRING TERMINATOR
    assert_eq!(
        format!("{:?}", cmd),
        "Exec { sh arg1 'don'\\''t' 'arg3 arg4' '?' ' ' '\u{009c}' }"
    );
    let cmd = cmd.env("foo", "bar");
    assert_eq!(
        format!("{:?}", cmd),
        "Exec { foo=bar sh arg1 'don'\\''t' 'arg3 arg4' '?' ' ' '\u{009c}' }"
    );
    let cmd = cmd.env("bar", "baz");
    assert_eq!(
        format!("{:?}", cmd),
        "Exec { foo=bar bar=baz sh arg1 'don'\\''t' 'arg3 arg4' '?' ' ' '\u{009c}' }"
    );
    let cmd = cmd.env_clear();
    assert_eq!(
        format!("{:?}", cmd),
        format!(
            "Exec {{ {} sh arg1 'don'\\''t' 'arg3 arg4' '?' ' ' '\u{009c}' }}",
            env::vars()
                .map(|(k, _)| format!("{}=", Exec::display_escape(&k)))
                .collect::<Vec<_>>()
                .join(" ")
        )
    );
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
fn capture_timeout() {
    match Exec::cmd("sleep")
        .args(&["0.5"])
        .capture_timeout(Duration::from_millis(100))
        .capture()
    {
        Ok(_) => panic!("expected timeout return"),
        Err(e) => match e.kind() {
            ErrorKind::TimedOut => assert!(true),
            _ => panic!("expected timeout return"),
        },
    }
}

#[test]
fn pipeline_capture_timeout() {
    match (Exec::cmd("sleep").arg("0.5") | Exec::cmd("cat"))
        .capture_timeout(Duration::from_millis(100))
        .capture()
    {
        Ok(_) => panic!("expected timeout return"),
        Err(e) => assert_eq!(e.kind(), ErrorKind::TimedOut),
    }
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
fn pipeline_stderr_all_pipe_popen_errors() {
    // Pipe without capture/communicate is not supported.
    let result = (Exec::cmd("true") | Exec::cmd("true"))
        .stderr_all(Redirection::Pipe)
        .popen();
    assert_eq!(result.unwrap_err().kind(), ErrorKind::InvalidInput);
}
