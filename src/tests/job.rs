use std::fs::{self, File};
use std::io::{self, ErrorKind, prelude::*};
use std::time::{Duration, Instant};

use tempfile::TempDir;

use crate::{Exec, Redirection};

// --- Single-command Job tests ---

#[test]
fn exec_start() {
    let mut handle = Exec::cmd("echo")
        .arg("hello")
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    let output = io::read_to_string(handle.stdout.take().unwrap()).unwrap();
    assert!(output.contains("hello"));
}

#[test]
fn exec_start_capture() {
    let c = Exec::cmd("echo")
        .arg("hello")
        .stdout(Redirection::Pipe)
        .start()
        .unwrap()
        .capture()
        .unwrap();
    assert!(c.stdout_str().contains("hello"));
}

#[test]
fn exec_start_join() {
    let status = Exec::cmd("true").start().unwrap().join().unwrap();
    assert!(status.success());

    let status = Exec::cmd("false").start().unwrap().join().unwrap();
    assert!(!status.success());
}

#[test]
fn exec_start_stdin_write() {
    let mut handle = Exec::cmd("cat")
        .stdin(Redirection::Pipe)
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    handle
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"hello world")
        .unwrap();
    handle.stdin.take(); // close stdin to let cat finish
    let output = io::read_to_string(handle.stdout.take().unwrap()).unwrap();
    assert_eq!(output, "hello world");
}

#[test]
fn exec_start_stderr() {
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "echo err-output >&2"])
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let stderr = io::read_to_string(handle.stderr.take().unwrap()).unwrap();
    assert_eq!(stderr.trim(), "err-output");
}

#[test]
fn exec_start_stdin_data_capture() {
    // stdin_data set via .stdin("data") is moved into Job and correctly
    // fed through communicate in the capture path.
    let c = Exec::cmd("cat")
        .stdin("hello from stdin_data")
        .stdout(Redirection::Pipe)
        .start()
        .unwrap()
        .capture()
        .unwrap();
    assert_eq!(c.stdout_str(), "hello from stdin_data");
}

#[test]
fn read_from_stdout() {
    let mut handle = Exec::cmd("echo")
        .arg("foo")
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    assert_eq!(
        io::read_to_string(handle.stdout.take().unwrap()).unwrap(),
        "foo\n"
    );
    assert!(handle.wait().unwrap().success());
}

#[test]
fn input_from_file() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("input");
    {
        let mut outfile = File::create(&tmpname).unwrap();
        outfile.write_all(b"foo").unwrap();
    }
    let mut handle = Exec::cmd("cat")
        .arg(tmpname.to_str().unwrap())
        .stdin(File::open(&tmpname).unwrap())
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    assert_eq!(
        io::read_to_string(handle.stdout.take().unwrap()).unwrap(),
        "foo"
    );
    assert!(handle.wait().unwrap().success());
}

#[test]
fn output_to_file() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("output");
    let outfile = File::create(&tmpname).unwrap();
    let status = Exec::cmd("printf")
        .arg("foo")
        .stdout(outfile)
        .start()
        .unwrap()
        .wait()
        .unwrap();
    assert!(status.success());
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
    let status = Exec::cmd("cat")
        .stdin(File::open(&tmpname_in).unwrap())
        .stdout(File::create(&tmpname_out).unwrap())
        .start()
        .unwrap()
        .wait()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read_to_string(&tmpname_out).unwrap(), "foo");
}

#[test]
fn write_to_subprocess() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("output");
    let mut handle = Exec::cmd("uniq")
        .args(&["-", tmpname.to_str().unwrap()])
        .stdin(Redirection::Pipe)
        .start()
        .unwrap();
    handle
        .stdin
        .take()
        .unwrap()
        .write_all(b"foo\nfoo\nbar\n")
        .unwrap();
    assert!(handle.wait().unwrap().success());
    assert_eq!(fs::read_to_string(tmpname).unwrap(), "foo\nbar\n");
}

#[test]
fn merge_err_to_out_pipe() {
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "echo foo; echo bar >&2"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Merge)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().read().unwrap();
    assert_eq!(out, b"foo\nbar\n");
    assert!(err.is_empty());
    assert!(handle.wait().unwrap().success());
}

#[test]
fn merge_out_to_err_pipe() {
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "echo foo; echo bar >&2"])
        .stdout(Redirection::Merge)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().read().unwrap();
    assert!(out.is_empty());
    assert_eq!(err, b"foo\nbar\n");
    assert!(handle.wait().unwrap().success());
}

#[test]
fn merge_err_to_out_file() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("output");
    let status = Exec::cmd("sh")
        .args(&["-c", "printf foo; printf bar >&2"])
        .stdout(File::create(&tmpname).unwrap())
        .stderr(Redirection::Merge)
        .start()
        .unwrap()
        .wait()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read_to_string(&tmpname).unwrap(), "foobar");
}

#[test]
fn broken_pipe_on_stdin() {
    // Child exits immediately without reading stdin
    let mut handle = Exec::cmd("true").stdin(Redirection::Pipe).start().unwrap();
    // Try to write data - the child exits without reading, causing
    // broken pipe
    let large_data = vec![0u8; 100_000];
    // Write may succeed or fail with BrokenPipe, but must not hang
    let _ = handle.stdin.as_mut().unwrap().write_all(&large_data);
    drop(handle.stdin.take());
    // Process should still be waitable
    handle.wait().unwrap();
}

// --- Process lifecycle tests (via Job) ---

#[test]
fn terminate() {
    let handle = Exec::cmd("sleep").arg("1000").start().unwrap();
    handle.terminate().unwrap();
    handle.wait().unwrap();
}

#[test]
fn terminate_twice() {
    use std::thread;
    use std::time::Duration;

    let handle = Exec::cmd("sleep").arg("1000").start().unwrap();
    handle.terminate().unwrap();
    thread::sleep(Duration::from_millis(100));
    handle.terminate().unwrap();
}

#[test]
fn terminate_after_exit() {
    let handle = Exec::cmd("true").start().unwrap();
    handle.processes[0].wait().unwrap();
    // Should be no-op, not error
    handle.processes[0].terminate().unwrap();
    handle.processes[0].kill().unwrap();
}

#[test]
fn pid_while_running() {
    let handle = Exec::cmd("sleep").arg("10").start().unwrap();
    // pid() returns u32 always, verify it is nonzero while running
    assert!(
        handle.processes[0].pid() > 0,
        "pid() should be nonzero while running"
    );
    assert!(
        handle.processes[0].exit_status().is_none(),
        "exit_status() should be None while running"
    );
    handle.processes[0].terminate().unwrap();
    handle.processes[0].wait().unwrap();
    // pid is still available after exit
    assert!(
        handle.processes[0].pid() > 0,
        "pid() should still be nonzero after exit"
    );
    assert!(
        handle.processes[0].exit_status().is_some(),
        "exit_status() should be Some after exit"
    );
}

#[test]
fn poll_running_process() {
    let handle = Exec::cmd("sleep").arg("10").start().unwrap();
    assert!(
        handle.processes[0].poll().is_none(),
        "poll() should return None for running process"
    );
    handle.processes[0].terminate().unwrap();
    handle.processes[0].wait().unwrap();
    assert!(
        handle.processes[0].poll().is_some(),
        "poll() should return Some after process finished"
    );
}

#[test]
fn poll_finished_process() {
    let handle = Exec::cmd("true").start().unwrap();
    handle.processes[0].wait().unwrap();
    assert!(handle.processes[0].poll().unwrap().success());
    // Multiple polls should return the same result
    assert!(handle.processes[0].poll().unwrap().success());
}

#[test]
fn wait_multiple_times() {
    let handle = Exec::cmd("sh").args(&["-c", "exit 42"]).start().unwrap();
    let s1 = handle.processes[0].wait().unwrap();
    let s2 = handle.processes[0].wait().unwrap();
    let s3 = handle.processes[0].wait().unwrap();
    assert_eq!(s1.code(), Some(42));
    assert_eq!(s1, s2);
    assert_eq!(s2, s3);
}

#[test]
fn wait_timeout() {
    let handle = Exec::cmd("sleep").arg("0.5").start().unwrap();
    let ret = handle.processes[0]
        .wait_timeout(Duration::from_millis(100))
        .unwrap();
    assert!(ret.is_none());
    // Sleep for a very long time to avoid flaky failures when we get a
    // slow machine that takes too long to start sleep(1).
    let ret = handle.processes[0]
        .wait_timeout(Duration::from_millis(900))
        .unwrap();
    assert!(ret.unwrap().success());
}

#[test]
fn wait_timeout_zero() {
    let handle = Exec::cmd("sleep").arg("10").start().unwrap();
    // Zero timeout should return immediately
    let start = Instant::now();
    let result = handle.processes[0].wait_timeout(Duration::ZERO).unwrap();
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "zero timeout took too long"
    );
    assert!(result.is_none());
    handle.processes[0].terminate().unwrap();
    handle.processes[0].wait().unwrap();
}

#[test]
fn wait_timeout_already_finished() {
    let handle = Exec::cmd("true").start().unwrap();
    handle.processes[0].wait().unwrap();
    // Timeout on finished process should return immediately with cached
    // status
    let start = Instant::now();
    let result = handle.processes[0]
        .wait_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "wait_timeout on finished process took too long"
    );
    assert!(result.unwrap().success());
}

#[test]
fn detach_does_not_wait_on_drop() {
    let start = Instant::now();
    {
        let handle = Exec::cmd("sleep").arg("10").detached().start().unwrap();
        // handle and its processes are dropped here without waiting
        drop(handle);
    }
    // Should return almost immediately, not wait 10 seconds
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "detach() didn't prevent waiting on drop"
    );
}

// --- Job timeout tests ---

#[test]
fn capture_timeout() {
    match Exec::cmd("sleep")
        .args(&["0.5"])
        .start()
        .unwrap()
        .capture_timeout(Duration::from_millis(100))
    {
        Ok(_) => panic!("expected timeout return"),
        Err(e) => match e.kind() {
            ErrorKind::TimedOut => assert!(true),
            _ => panic!("expected timeout return"),
        },
    }
}

#[test]
fn exec_timeout_join_timed_out() {
    let result = Exec::cmd("sleep")
        .arg("0.5")
        .start()
        .unwrap()
        .join_timeout(Duration::from_millis(100));
    assert_eq!(result.unwrap_err().kind(), ErrorKind::TimedOut);
}

#[test]
fn exec_timeout_join_succeeds() {
    let status = Exec::cmd("true")
        .start()
        .unwrap()
        .join_timeout(Duration::from_secs(5))
        .unwrap();
    assert!(status.success());
}

#[test]
fn exec_wait_timeout_terminate() {
    let started = Exec::cmd("sleep").arg("10").start().unwrap();
    let result = started.wait_timeout(Duration::from_millis(100)).unwrap();
    assert!(result.is_none());
    started.terminate().unwrap();
    let status = started.wait().unwrap();
    assert!(!status.success());
}

// --- Job convenience method tests ---

#[test]
fn started_pid() {
    let handle = Exec::cmd("sleep").arg("10").start().unwrap();
    assert!(handle.pid() > 0, "pid() should be nonzero");
    handle.processes[0].terminate().unwrap();
    handle.processes[0].wait().unwrap();
}

#[test]
fn started_kill() {
    let handle = Exec::cmd("sleep").arg("1000").start().unwrap();
    handle.kill().unwrap();
    let status = handle.wait().unwrap();
    assert!(!status.success());
}

#[test]
fn started_poll() {
    let handle = Exec::cmd("sleep").arg("10").start().unwrap();
    assert!(
        handle.poll().is_none(),
        "poll() should be None while running"
    );
    handle.processes[0].terminate().unwrap();
    handle.processes[0].wait().unwrap();
    assert!(
        handle.poll().is_some(),
        "poll() should be Some after finished"
    );
}

#[test]
fn started_wait_timeout_none() {
    let handle = Exec::cmd("sleep").arg("10").start().unwrap();
    let result = handle.wait_timeout(Duration::from_millis(100)).unwrap();
    assert!(result.is_none(), "should return None on timeout");
    handle.terminate().unwrap();
    handle.wait().unwrap();
}

#[test]
fn started_wait_timeout_some() {
    let handle = Exec::cmd("true").start().unwrap();
    let result = handle.wait_timeout(Duration::from_secs(5)).unwrap();
    assert!(result.is_some(), "should return Some when done");
    assert!(result.unwrap().success());
}

// --- Pipeline Job tests ---

#[test]
fn pipeline_detached() {
    let start = Instant::now();
    {
        let _handle = { Exec::cmd("sleep").arg("10") | Exec::cmd("sleep").arg("10") }
            .detached()
            .start()
            .unwrap();
        // handle and its processes are dropped here without waiting
    }
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "detached() didn't prevent waiting on drop"
    );
}

#[test]
fn pipeline_start() {
    let mut handle = { Exec::cmd("echo").arg("foo\nbar") | Exec::cmd("wc").arg("-l") }
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    let output = io::read_to_string(handle.stdout.take().unwrap()).unwrap();
    assert_eq!(output.trim(), "2");
}

#[test]
fn pipeline_start_processes_accessible() {
    let handle = { Exec::cmd("echo").arg("foo") | Exec::cmd("cat") }
        .start()
        .unwrap();
    let status = handle.processes.last().unwrap().wait().unwrap();
    assert!(status.success());
}

#[test]
fn pipeline_start_join() {
    let status = { Exec::cmd("echo").arg("hi") | Exec::cmd("cat") }
        .start()
        .unwrap()
        .join()
        .unwrap();
    assert!(status.success());
}

#[test]
fn pipeline_start_stdin_write() {
    let mut handle = { Exec::cmd("cat") | Exec::cmd("cat") }
        .stdin(Redirection::Pipe)
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    handle
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"piped data")
        .unwrap();
    handle.stdin.take(); // close stdin
    let output = io::read_to_string(handle.stdout.take().unwrap()).unwrap();
    assert_eq!(output, "piped data");
}

#[test]
fn pipeline_start_stdin_data_capture() {
    // stdin_data flows through pipeline's start+capture path.
    let c = { Exec::cmd("cat") | Exec::cmd("cat") }
        .stdin("hello from pipeline")
        .stdout(Redirection::Pipe)
        .start()
        .unwrap()
        .capture()
        .unwrap();
    assert_eq!(c.stdout_str(), "hello from pipeline");
}

#[test]
fn pipeline_start_capture_no_pipes() {
    // start() does no auto-setup, so without explicit pipes, capture() gets
    // nothing - process output goes to the parent's stdout (inherited).
    let c = { Exec::cmd("echo").arg("hello") | Exec::cmd("cat") }
        .start()
        .unwrap()
        .capture()
        .unwrap();
    assert_eq!(c.stdout_str(), "");
    assert_eq!(c.stderr_str(), "");
}

#[test]
fn pipeline_start_capture_stdout_only() {
    // With start(), only explicitly set pipes produce data.  Here stdout is
    // piped but stderr is not, so stderr is empty.  Compare to
    // pipeline.capture() which would auto-set stderr_all(Pipe).
    let c = { Exec::cmd("sh").arg("-c").arg("echo out; echo err >&2") | Exec::cmd("cat") }
        .stdout(Redirection::Pipe)
        .start()
        .unwrap()
        .capture()
        .unwrap();
    assert!(
        c.stdout_str().contains("out"),
        "stdout should contain 'out', got: {:?}",
        c.stdout_str()
    );
    assert_eq!(c.stderr_str(), "");
}

#[test]
fn pipeline_start_communicate_needs_explicit_pipes() {
    // start() doesn't auto-set pipes - you must configure them explicitly.
    // Here we set stdout(Pipe) and verify the communicator reads from it.
    let mut handle = { Exec::cmd("echo").arg("foo") | Exec::cmd("cat") }
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    let mut comm = handle.communicate();
    let (stdout, stderr) = comm.read().unwrap();
    assert_eq!(String::from_utf8_lossy(&stdout).trim(), "foo");
    assert_eq!(stderr, b"");
}

#[test]
fn pipeline_stderr_all_pipe_start() {
    // stderr(Pipe) with start() provides the shared stderr read end.
    let mut handle = {
        Exec::cmd("sh").arg("-c").arg("echo err1 >&2; echo out1")
            | Exec::cmd("sh").arg("-c").arg("cat; echo err2 >&2")
    }
    .stdout(Redirection::Pipe)
    .stderr_all(Redirection::Pipe)
    .start()
    .unwrap();

    let stdout = io::read_to_string(handle.stdout.take().unwrap()).unwrap();
    let stderr = io::read_to_string(handle.stderr.take().unwrap()).unwrap();
    assert!(stdout.contains("out1"), "stdout: {:?}", stdout);
    assert!(stderr.contains("err1"), "stderr: {:?}", stderr);
    assert!(stderr.contains("err2"), "stderr: {:?}", stderr);
}

#[test]
fn pipeline_capture_timeout() {
    match (Exec::cmd("sleep").arg("0.5") | Exec::cmd("cat"))
        .start()
        .unwrap()
        .capture_timeout(Duration::from_millis(100))
    {
        Ok(_) => panic!("expected timeout return"),
        Err(e) => assert_eq!(e.kind(), ErrorKind::TimedOut),
    }
}

#[test]
fn pipeline_timeout_join_timed_out() {
    let result = (Exec::cmd("sleep").arg("0.5") | Exec::cmd("cat"))
        .start()
        .unwrap()
        .join_timeout(Duration::from_millis(100));
    assert_eq!(result.unwrap_err().kind(), ErrorKind::TimedOut);
}
