// note that these tests run on Windows despite using `sh` and such - those Unix commands
// are expected to be present in Windows CI.

use tempfile::TempDir;

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{ExitStatus, Popen, PopenConfig, Redirection};

#[test]
fn good_cmd() {
    let mut p = Popen::create(&["true"], PopenConfig::default()).unwrap();
    assert!(p.wait().unwrap().success());
}

#[test]
fn bad_cmd() {
    let result = Popen::create(&["nosuchcommand"], PopenConfig::default());
    assert!(result.is_err());
}

#[test]
fn reject_empty_argv() {
    let test = Popen::create(&[""; 0], PopenConfig::default());
    assert!(
        matches!(&test, Err(e) if e.kind() == io::ErrorKind::InvalidInput),
        "didn't get InvalidInput for empty argv"
    );
}

#[test]
fn err_exit() {
    let mut p = Popen::create(&["sh", "-c", "exit 13"], PopenConfig::default()).unwrap();
    assert_eq!(p.wait().unwrap(), ExitStatus::Exited(13));
}

#[test]
fn terminate() {
    let mut p = Popen::create(&["sleep", "1000"], PopenConfig::default()).unwrap();
    p.terminate().unwrap();
    p.wait().unwrap();
}

#[test]
fn terminate_twice() {
    use std::thread;
    use std::time::Duration;

    let mut p = Popen::create(&["sleep", "1000"], PopenConfig::default()).unwrap();
    p.terminate().unwrap();
    thread::sleep(Duration::from_millis(100));
    p.terminate().unwrap();
}

#[test]
fn read_from_stdout() {
    let mut p = Popen::create(
        &["echo", "foo"],
        PopenConfig {
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        io::read_to_string(p.stdout.take().unwrap()).unwrap(),
        "foo\n"
    );
    assert!(p.wait().unwrap().success());
}

#[test]
fn input_from_file() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("input");
    {
        let mut outfile = File::create(&tmpname).unwrap();
        outfile.write_all(b"foo").unwrap();
    }
    let mut p = Popen::create(
        &["cat", tmpname.to_str().unwrap()],
        PopenConfig {
            stdin: Redirection::File(File::open(&tmpname).unwrap()),
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(io::read_to_string(p.stdout.take().unwrap()).unwrap(), "foo");
    assert!(p.wait().unwrap().success());
}

#[test]
fn output_to_file() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("output");
    let outfile = File::create(&tmpname).unwrap();
    let mut p = Popen::create(
        &["printf", "foo"],
        PopenConfig {
            stdout: Redirection::File(outfile),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(p.wait().unwrap().success());
    assert_eq!(fs::read_to_string(&tmpname).unwrap(), "foo");
}

#[test]
fn input_output_from_file() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname_in = tmpdir.path().join("input");
    let tmpname_out = tmpdir.path().join("output");
    {
        let mut f = File::create(&tmpname_in).unwrap();
        f.write_all(b"foo").unwrap();
    }
    let mut p = Popen::create(
        &["cat"],
        PopenConfig {
            stdin: Redirection::File(File::open(&tmpname_in).unwrap()),
            stdout: Redirection::File(File::create(&tmpname_out).unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(p.wait().unwrap().success());
    assert_eq!(fs::read_to_string(&tmpname_out).unwrap(), "foo");
}

#[test]
fn write_to_subprocess() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("output");
    let mut p = Popen::create(
        &[r"uniq", "-", tmpname.to_str().unwrap()],
        PopenConfig {
            stdin: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    p.stdin
        .take()
        .unwrap()
        .write_all(b"foo\nfoo\nbar\n")
        .unwrap();
    assert_eq!(p.wait().unwrap(), ExitStatus::Exited(0));
    assert_eq!(fs::read_to_string(tmpname).unwrap(), "foo\nbar\n");
}

#[test]
fn null_byte_in_cmd() {
    let try_p = Popen::create(&["echo\0foo"], PopenConfig::default());
    assert!(try_p.is_err());
}

#[test]
fn merge_err_to_out_pipe() {
    let mut p = Popen::create(
        &["sh", "-c", "echo foo; echo bar >&2"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Merge,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate_bytes(None).unwrap();
    assert_eq!(out, b"foo\nbar\n");
    assert!(err.is_empty());
    assert!(p.wait().unwrap().success());
}

#[test]
fn merge_out_to_err_pipe() {
    let mut p = Popen::create(
        &["sh", "-c", "echo foo; echo bar >&2"],
        PopenConfig {
            stdout: Redirection::Merge,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate_bytes(None).unwrap();
    assert!(out.is_empty());
    assert_eq!(err, b"foo\nbar\n");
    assert!(p.wait().unwrap().success());
}

#[test]
fn merge_err_to_out_file() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("output");
    let mut p = Popen::create(
        &["sh", "-c", "printf foo; printf bar >&2"],
        PopenConfig {
            stdout: Redirection::File(File::create(&tmpname).unwrap()),
            stderr: Redirection::Merge,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(p.wait().unwrap().success());
    assert_eq!(fs::read_to_string(&tmpname).unwrap(), "foobar");
}

#[test]
fn simple_pipe() {
    let mut c1 = Popen::create(
        &["printf", "foo\\nbar\\nbaz\\n"],
        PopenConfig {
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let mut c2 = Popen::create(
        &["wc", "-l"],
        PopenConfig {
            stdin: Redirection::File(c1.stdout.take().unwrap()),
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (wcout, _) = c2.communicate(None).unwrap();
    assert_eq!(wcout.trim(), "3");
}

#[test]
fn wait_timeout() {
    let mut p = Popen::create(&["sleep", "0.5"], PopenConfig::default()).unwrap();
    let ret = p.wait_timeout(Duration::from_millis(100)).unwrap();
    assert!(ret.is_none());
    // We sleep for a very long time to avoid flaky failures when we get a slow machine
    // that takes too long to start sleep(1).
    let ret = p.wait_timeout(Duration::from_millis(900)).unwrap();
    assert_eq!(ret, Some(ExitStatus::Exited(0)));
}

#[test]
fn env_add() {
    let mut env = PopenConfig::current_env();
    env.push((OsString::from("SOMEVAR"), OsString::from("foo")));
    let mut p = Popen::create(
        &["sh", "-c", r#"test "$SOMEVAR" = "foo""#],
        PopenConfig {
            env: Some(env),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(p.wait().unwrap().success());
}

#[test]
fn env_dup() {
    let dups = vec![
        (OsString::from("SOMEVAR"), OsString::from("foo")),
        (OsString::from("SOMEVAR"), OsString::from("bar")),
    ];
    let mut p = Popen::create(
        &["sh", "-c", r#"test "$SOMEVAR" = "bar""#],
        PopenConfig {
            stdout: Redirection::Pipe,
            env: Some(dups),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(p.wait().unwrap().success());
}

#[test]
fn cwd() {
    let tmpdir = TempDir::new().unwrap();
    let tmpdir_name = tmpdir.path().as_os_str().to_owned();

    // Test that CWD works by cwd-ing into an empty temporary directory and creating a
    // file there.  Trying to print the directory's name and compare it to tmpdir fails
    // due to MinGW interference on Windows and symlinks on Unix.

    Popen::create(
        &["touch", "here"],
        PopenConfig {
            stdout: Redirection::Pipe,
            cwd: Some(tmpdir_name),
            ..Default::default()
        },
    )
    .unwrap();

    assert!(tmpdir.path().join("here").exists());
}

#[test]
fn failed_cwd() {
    let ret = Popen::create(
        &["anything"],
        PopenConfig {
            stdout: Redirection::Pipe,
            cwd: Some("/nosuchdir".into()),
            ..Default::default()
        },
    );
    let err_num = match ret {
        Err(e) => e.raw_os_error().unwrap_or(-1),
        _ => panic!("expected error return"),
    };
    assert_eq!(err_num, libc::ENOENT);
}

#[test]
fn detach_does_not_wait_on_drop() {
    let start = Instant::now();
    {
        let mut p = Popen::create(&["sleep", "10"], PopenConfig::default()).unwrap();
        p.detach();
        // p is dropped here without waiting
    }
    // Should return almost immediately, not wait 10 seconds
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "detach() didn't prevent waiting on drop"
    );
}

#[test]
fn poll_running_process() {
    let mut p = Popen::create(&["sleep", "10"], PopenConfig::default()).unwrap();
    assert!(
        p.poll().is_none(),
        "poll() should return None for running process"
    );
    p.terminate().unwrap();
    p.wait().unwrap();
    assert!(
        p.poll().is_some(),
        "poll() should return Some after process finished"
    );
}

#[test]
fn poll_finished_process() {
    let mut p = Popen::create(&["true"], PopenConfig::default()).unwrap();
    p.wait().unwrap();
    assert_eq!(p.poll(), Some(ExitStatus::Exited(0)));
    // Multiple polls should return the same result
    assert_eq!(p.poll(), Some(ExitStatus::Exited(0)));
}

#[test]
fn wait_multiple_times() {
    let mut p = Popen::create(&["sh", "-c", "exit 42"], PopenConfig::default()).unwrap();
    let s1 = p.wait().unwrap();
    let s2 = p.wait().unwrap();
    let s3 = p.wait().unwrap();
    assert_eq!(s1, ExitStatus::Exited(42));
    assert_eq!(s1, s2);
    assert_eq!(s2, s3);
}

#[test]
fn merge_on_stdin_rejected() {
    let result = Popen::create(
        &["true"],
        PopenConfig {
            stdin: Redirection::Merge,
            ..Default::default()
        },
    );
    assert!(
        matches!(&result, Err(e) if e.kind() == io::ErrorKind::InvalidInput),
        "Merge on stdin should be rejected"
    );
}

#[test]
fn merge_both_stdout_stderr_rejected() {
    let result = Popen::create(
        &["true"],
        PopenConfig {
            stdout: Redirection::Merge,
            stderr: Redirection::Merge,
            ..Default::default()
        },
    );
    assert!(
        matches!(&result, Err(e) if e.kind() == io::ErrorKind::InvalidInput),
        "Merge on both stdout and stderr should be rejected"
    );
}

#[test]
fn broken_pipe_on_stdin() {
    // Child exits immediately without reading stdin
    let mut p = Popen::create(
        &["true"],
        PopenConfig {
            stdin: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    // Try to write data - the child exits without reading, causing broken pipe
    let large_data = vec![0u8; 100_000];
    // Write may succeed or fail with BrokenPipe, but must not hang
    let _ = p.stdin.as_mut().unwrap().write_all(&large_data);
    drop(p.stdin.take());
    // Process should still be waitable
    p.wait().unwrap();
}

#[test]
fn shared_file_output() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("output");
    let file = Arc::new(File::create(&tmpname).unwrap());
    let mut p = Popen::create(
        &["sh", "-c", "printf out; printf err >&2"],
        PopenConfig {
            stdout: Redirection::SharedFile(Arc::clone(&file)),
            stderr: Redirection::SharedFile(file),
            ..Default::default()
        },
    )
    .unwrap();
    p.wait().unwrap();
    let content = fs::read_to_string(&tmpname).unwrap();
    // Both stdout and stderr should go to the same file
    assert!(content.contains("out"), "stdout missing from shared file");
    assert!(content.contains("err"), "stderr missing from shared file");
}

#[test]
fn pid_while_running() {
    let mut p = Popen::create(&["sleep", "10"], PopenConfig::default()).unwrap();
    assert!(p.pid().is_some(), "pid() should return Some while running");
    assert!(
        p.exit_status().is_none(),
        "exit_status() should be None while running"
    );
    p.terminate().unwrap();
    p.wait().unwrap();
    assert!(p.pid().is_none(), "pid() should return None after exit");
    assert!(
        p.exit_status().is_some(),
        "exit_status() should be Some after exit"
    );
}

#[test]
fn terminate_after_exit() {
    let mut p = Popen::create(&["true"], PopenConfig::default()).unwrap();
    p.wait().unwrap();
    // Should be no-op, not error
    p.terminate().unwrap();
    p.kill().unwrap();
}

#[test]
fn wait_timeout_zero() {
    let mut p = Popen::create(&["sleep", "10"], PopenConfig::default()).unwrap();
    // Zero timeout should return immediately
    let start = Instant::now();
    let result = p.wait_timeout(Duration::ZERO).unwrap();
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "zero timeout took too long"
    );
    assert!(result.is_none());
    p.terminate().unwrap();
    p.wait().unwrap();
}

#[test]
fn wait_timeout_already_finished() {
    let mut p = Popen::create(&["true"], PopenConfig::default()).unwrap();
    p.wait().unwrap();
    // Timeout on finished process should return immediately with cached status
    let start = Instant::now();
    let result = p.wait_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "wait_timeout on finished process took too long"
    );
    assert_eq!(result, Some(ExitStatus::Exited(0)));
}
