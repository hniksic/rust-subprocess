use std::env;
use std::io::{self, ErrorKind};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use crate::{Exec, Redirection};

#[test]
fn good_cmd() {
    let status = Exec::cmd("true").start().unwrap().wait().unwrap();
    assert!(status.success());
}

#[test]
fn bad_cmd() {
    let result = Exec::cmd("nosuchcommand").start();
    assert!(result.is_err());
}

#[test]
fn reject_empty_argv() {
    // Exec::cmd() always produces at least one argv element, so we
    // test the empty argv rejection at the spawn level directly.
    let test = crate::spawn::spawn(
        vec![],
        Redirection::None,
        Redirection::None,
        Redirection::None,
        false,
        None,
        None,
        None,
        #[cfg(unix)]
        None,
        #[cfg(unix)]
        None,
        #[cfg(unix)]
        None,
        #[cfg(windows)]
        0,
    );
    assert!(
        matches!(&test, Err(e) if e.kind() == io::ErrorKind::InvalidInput),
        "didn't get InvalidInput for empty argv"
    );
}

#[test]
fn err_exit() {
    let status = Exec::cmd("sh")
        .args(&["-c", "exit 13"])
        .start()
        .unwrap()
        .wait()
        .unwrap();
    assert_eq!(status.code(), Some(13));
}

#[test]
fn null_byte_in_cmd() {
    let try_p = Exec::cmd("echo\0foo").start();
    assert!(try_p.is_err());
}

#[test]
fn merge_on_stdin_rejected() {
    // Redirection::Merge on stdin panics in the InputRedirection impl
    // for Exec, so we test Merge on stdin at the spawn level directly.
    let result = crate::spawn::spawn(
        vec!["true".into()],
        Redirection::Merge,
        Redirection::None,
        Redirection::None,
        false,
        None,
        None,
        None,
        #[cfg(unix)]
        None,
        #[cfg(unix)]
        None,
        #[cfg(unix)]
        None,
        #[cfg(windows)]
        0,
    );
    assert!(
        matches!(&result, Err(e) if e.kind() == io::ErrorKind::InvalidInput),
        "Merge on stdin should be rejected"
    );
}

#[test]
fn merge_both_stdout_stderr_rejected() {
    let result = Exec::cmd("true")
        .stdout(Redirection::Merge)
        .stderr(Redirection::Merge)
        .start();
    assert!(
        matches!(&result, Err(e) if e.kind() == io::ErrorKind::InvalidInput),
        "Merge on both stdout and stderr should be rejected"
    );
}

#[test]
fn exec_join() {
    let status = Exec::cmd("true").join().unwrap();
    assert!(status.success());
}

#[test]
fn null_file() {
    let c = Exec::cmd("cat")
        .stdin(Redirection::Null)
        .stdout(Redirection::Pipe)
        .capture()
        .unwrap();
    assert_eq!(c.stdout_str(), "");
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
    use std::fs::{self, File};
    use std::io::prelude::*;
    use tempfile::TempDir;

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
fn capture_out_with_input_data_bytes() {
    let c = Exec::cmd("cat").stdin(b"foo" as &[u8]).capture().unwrap();
    assert_eq!(c.stdout_str(), "foo");
    let c = Exec::cmd("cat").stdin(b"bar").capture().unwrap();
    assert_eq!(c.stdout_str(), "bar");
}

#[test]
fn exec_shell() {
    let stream = Exec::shell("printf foo").stream_stdout().unwrap();
    assert_eq!(io::read_to_string(stream).unwrap(), "foo");
}

#[test]
fn join_with_input_data() {
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
fn env_add() {
    let status = Exec::cmd("sh")
        .args(&["-c", r#"test "$SOMEVAR" = "foo""#])
        .env("SOMEVAR", "foo")
        .start()
        .unwrap()
        .wait()
        .unwrap();
    assert!(status.success());
}

#[test]
fn env_dup() {
    let status = Exec::cmd("sh")
        .args(&["-c", r#"test "$SOMEVAR" = "bar""#])
        .env_clear()
        .env("SOMEVAR", "foo")
        .env("SOMEVAR", "bar")
        .stdout(Redirection::Pipe)
        .start()
        .unwrap()
        .wait()
        .unwrap();
    assert!(status.success());
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
fn exec_capture_auto_stdout_when_stderr_set() {
    // Exec::capture() auto-pipes stdout and stderr independently.
    // Setting stderr(Pipe) does not suppress stdout auto-piping.
    let c = Exec::cmd("sh")
        .args(&["-c", "echo out; echo err >&2"])
        .stderr(Redirection::Pipe)
        .capture()
        .unwrap();
    assert_eq!(c.stderr_str().trim(), "err");
    assert_eq!(c.stdout_str().trim(), "out");
}

#[test]
fn exec_capture_auto_pipes_both() {
    // Bare Exec::cmd(...).capture() auto-pipes both stdout and stderr.
    let c = Exec::cmd("sh")
        .args(&["-c", "echo out; echo err >&2"])
        .capture()
        .unwrap();
    assert_eq!(c.stdout_str().trim(), "out");
    assert_eq!(c.stderr_str().trim(), "err");
}

#[test]
fn exec_communicate_auto_stdout_when_stderr_set() {
    // Exec::communicate() auto-pipes stdout and stderr independently.
    // Setting stderr(Pipe) does not suppress stdout auto-piping.
    let mut comm = Exec::cmd("sh")
        .args(&["-c", "echo out; echo err >&2"])
        .stderr(Redirection::Pipe)
        .communicate()
        .unwrap();
    let (stdout, stderr) = comm.read().unwrap();
    assert_eq!(String::from_utf8_lossy(&stdout).trim(), "out");
    assert_eq!(String::from_utf8_lossy(&stderr).trim(), "err");
}

#[test]
fn exec_timeout_communicate_timed_out() {
    let result = Exec::cmd("sleep")
        .arg("0.5")
        .communicate()
        .unwrap()
        .limit_time(Duration::from_millis(100))
        .read();
    assert_eq!(result.unwrap_err().kind(), ErrorKind::TimedOut);
}

#[test]
fn checked_join_ok() {
    Exec::cmd("true").checked().join().unwrap();
}

#[test]
fn checked_join_fail() {
    let err = Exec::cmd("false").checked().join().unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Other);
    assert!(err.to_string().contains("command failed"), "{err}");
}

#[test]
fn checked_capture_ok() {
    let c = Exec::cmd("true").checked().capture().unwrap();
    assert!(c.success());
}

#[test]
fn checked_capture_fail() {
    let err = Exec::cmd("false").checked().capture().unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Other);
    assert!(err.to_string().contains("command failed"), "{err}");
}

#[test]
fn check_ok() {
    Exec::cmd("true").capture().unwrap().check().unwrap();
}

#[test]
fn check_fail() {
    let err = Exec::cmd("false").capture().unwrap().check().unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Other);
    assert!(err.to_string().contains("command failed"), "{err}");
}
