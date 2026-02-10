use std::fs;
use std::io;
use std::io::Read;
use std::time::Duration;

use tempfile::TempDir;

use crate::InputData;
use crate::{Exec, Redirection};

#[test]
fn communicate_input() {
    // Feed input data to stdin, redirect stdout to a file, and verify
    // the data arrives in the file.
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("input");
    let mut handle = Exec::cmd("cat")
        .stdin("hello world")
        .stdout(std::fs::File::create(&tmpname).unwrap())
        .start()
        .unwrap();
    handle.communicate().unwrap().read().unwrap();
    assert!(handle.wait().unwrap().success());
    assert_eq!(fs::read_to_string(&tmpname).unwrap(), "hello world");
}

#[test]
fn communicate_output() {
    // Capture both stdout and stderr from a command that writes to both.
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "echo foo; echo bar >&2"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert_eq!(out, b"foo\n");
    assert_eq!(err, b"bar\n");
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_input_output() {
    // Feed input data and capture both stdout and stderr.
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "cat; echo foo >&2"])
        .stdin("hello world")
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert_eq!(out, b"hello world");
    assert_eq!(err, b"foo\n");
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_input_output_long() {
    // Large data in both directions with simultaneous stdout and stderr
    // output, testing deadlock prevention.
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "cat; printf '%100000s' '' >&2"])
        .stdin(vec![65u8; 1_000_000])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert_eq!(&out[..], &[65u8; 1_000_000][..]);
    assert_eq!(&err[..], &[32u8; 100_000][..]);
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_timeout() {
    // A command that produces partial output then sleeps should time out,
    // and the partial output should still be available.
    let mut job = Exec::cmd("sh")
        .args(&["-c", "printf foo; sleep 1"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let mut out = vec![];
    let mut err = vec![];
    let result = job
        .communicate()
        .unwrap()
        .limit_time(Duration::from_millis(100))
        .read_to(&mut out, &mut err);
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert_eq!(out, b"foo");
    assert_eq!(err, vec![]);
    job.kill().unwrap();
}

#[test]
fn communicate_size_limit_small() {
    // Read with a small size limit, then continue reading in chunks.
    let mut job = Exec::cmd("sh")
        .args(&["-c", "printf '%5s' a"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let mut comm = job.communicate().unwrap().limit_size(2);
    assert_eq!(comm.read().unwrap(), (vec![32; 2], vec![]));
    assert_eq!(comm.read().unwrap(), (vec![32; 2], vec![]));
    assert_eq!(comm.read().unwrap(), (vec![b'a'], vec![]));
    job.kill().unwrap();
}

fn check_vec(v: &[u8], size: usize, content: u8) {
    assert_eq!(v.len(), size);
    assert!(v.iter().all(|&c| c == content));
}

#[test]
fn communicate_size_limit_large() {
    // Read large output in chunks using limit_size.
    let mut job = Exec::cmd("sh")
        .args(&["-c", "printf '%20001s' a"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let mut comm = job.communicate().unwrap().limit_size(10_000);

    let (out, err) = comm.read().unwrap();
    check_vec(&out, 10_000, 32);
    assert_eq!(err, vec![]);

    let (out, err) = comm.read().unwrap();
    check_vec(&out, 10_000, 32);
    assert_eq!(err, vec![]);

    assert_eq!(comm.read().unwrap(), (vec![b'a'], vec![]));
    job.kill().unwrap();
}

#[test]
fn communicate_size_limit_different_sizes() {
    // Change the size limit between successive reads to verify that
    // the communicator respects the new limit each time.
    let mut job = Exec::cmd("sh")
        .args(&["-c", "printf '%20001s' a"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let comm = job.communicate().unwrap();

    let mut comm = comm.limit_size(100);
    let (out, err) = comm.read().unwrap();
    check_vec(&out, 100, 32);
    assert_eq!(err, vec![]);

    let mut comm = comm.limit_size(1_000);
    let (out, err) = comm.read().unwrap();
    check_vec(&out, 1_000, 32);
    assert_eq!(err, vec![]);

    let mut comm = comm.limit_size(10_000);
    let (out, err) = comm.read().unwrap();
    check_vec(&out, 10_000, 32);
    assert_eq!(err, vec![]);

    let mut comm = comm.limit_size(8_900);
    let (out, err) = comm.read().unwrap();
    check_vec(&out, 8_900, 32);
    assert_eq!(err, vec![]);

    assert_eq!(comm.read().unwrap(), (vec![b'a'], vec![]));
    assert_eq!(comm.read().unwrap(), (vec![], vec![]));
    job.kill().unwrap();
}

#[test]
fn communicate_stdout_only() {
    // Capture only stdout (no stderr pipe). Stderr output goes to the
    // parent's stderr and is not captured.
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "echo hello; echo ignored >&2"])
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert_eq!(out, b"hello\n");
    assert!(err.is_empty());
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_stderr_only() {
    // Capture only stderr (no stdout pipe). Stdout output goes to the
    // parent's stdout and is not captured.
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "echo ignored; echo error >&2"])
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert!(out.is_empty());
    assert_eq!(err, b"error\n");
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_stdin_only() {
    // Feed stdin data to a process with no output pipes. Output goes
    // to /dev/null equivalent (no pipe set up).
    let mut handle = Exec::cmd("cat").stdin("test data").start().unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_empty_input() {
    // Send empty input to stdin and verify cat produces empty output.
    let mut handle = Exec::cmd("cat")
        .stdin("")
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_empty_output() {
    // A process that produces no output should return empty vectors.
    let mut handle = Exec::cmd("true")
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_large_stderr() {
    // Test large output on stderr specifically.
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "printf '%50000s' x >&2"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert!(out.is_empty());
    assert_eq!(err.len(), 50000);
    assert!(err.iter().all(|&c| c == b' ' || c == b'x'));
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_interleaved_output() {
    // Test interleaved stdout/stderr - both should be captured correctly
    // in their respective buffers.
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "echo out1; echo err1 >&2; echo out2; echo err2 >&2"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert_eq!(out, b"out1\nout2\n");
    assert_eq!(err, b"err1\nerr2\n");
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_quick_exit() {
    // Process exits immediately without producing output.
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "exit 0"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_process_fails() {
    // Process exits with non-zero status. Communicate should still
    // succeed and return the captured data.
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "echo output; echo error >&2; exit 42"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert_eq!(out, b"output\n");
    assert_eq!(err, b"error\n");
    assert_eq!(handle.wait().unwrap().code(), Some(42));
}

#[test]
fn communicate_size_limit_zero() {
    // Size limit of 0 should return empty immediately; continue reading
    // with a larger limit to get the remaining data.
    let mut job = Exec::cmd("sh")
        .args(&["-c", "printf 'data'"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let mut comm = job.communicate().unwrap().limit_size(0);
    let (out, err) = comm.read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    // Continue reading without limit to get the rest
    let mut comm = comm.limit_size(100);
    let (out, err) = comm.read().unwrap();
    assert_eq!(out, b"data");
    assert_eq!(err, vec![]);
    job.kill().unwrap();
}

#[test]
fn communicate_size_limit_stderr() {
    // Size limit should apply to the combined total of stdout + stderr.
    let mut job = Exec::cmd("sh")
        .args(&["-c", "printf out; printf err >&2"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let mut comm = job.communicate().unwrap().limit_size(4);
    let (out, err) = comm.read().unwrap();
    // Should get approximately 4 bytes total across both streams
    let total = out.len() + err.len();
    assert!(total <= 6, "got {} bytes, expected <= 6", total);
    job.kill().unwrap();
}

#[test]
fn communicate_timeout_zero() {
    // Immediate timeout (zero duration) on a sleeping process.
    let mut job = Exec::cmd("sh")
        .args(&["-c", "sleep 1; echo done"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let result = job
        .communicate()
        .unwrap()
        .limit_time(Duration::from_secs(0))
        .read();
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    job.kill().unwrap();
}

#[test]
fn communicate_multiple_reads_after_eof() {
    // After EOF, subsequent reads should return empty data.
    let mut handle = Exec::cmd("printf")
        .arg("hello")
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();
    let mut comm = handle.communicate().unwrap();
    let (out, err) = comm.read().unwrap();
    assert_eq!(out, b"hello");
    assert!(err.is_empty());

    // Subsequent reads should return empty
    let (out, err) = comm.read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());

    // And again
    let (out, err) = comm.read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());

    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_large_bidirectional() {
    // Large data in both directions simultaneously - tests deadlock
    // prevention. 500KB is larger than the typical pipe buffer (64KB on
    // Linux).
    let input: Vec<u8> = (0..500_000).map(|i| (i % 256) as u8).collect();
    let mut handle = Exec::cmd("cat")
        .stdin(input.clone())
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, _) = handle.communicate().unwrap().read().unwrap();
    assert_eq!(out, input);
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_partial_read_continue() {
    // Read with a size limit, then continue reading in multiple chunks
    // until all data is consumed.
    let mut job = Exec::cmd("sh")
        .args(&["-c", "printf 'abcdefghij'"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();

    let mut comm = job.communicate().unwrap().limit_size(3);
    let (out1, _) = comm.read().unwrap();
    assert_eq!(out1.len(), 3);

    let mut comm = comm.limit_size(3);
    let (out2, _) = comm.read().unwrap();
    assert_eq!(out2.len(), 3);

    // Read the rest
    let mut comm = comm.limit_size(100);
    let (out3, _) = comm.read().unwrap();

    // Combine all reads
    let mut combined = out1;
    combined.extend(out2);
    combined.extend(out3);
    assert_eq!(combined, b"abcdefghij");

    job.kill().unwrap();
}

#[test]
fn communicate_no_streams() {
    // No pipes at all - should work fine, just returning empty data.
    let mut handle = Exec::cmd("true").start().unwrap();
    let (out, err) = handle.communicate().unwrap().read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_very_long_lines() {
    // Test with very long output that contains no newlines.
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "printf '%100000s' x"])
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, _) = handle.communicate().unwrap().read().unwrap();
    assert_eq!(out.len(), 100_000);
    assert!(out.ends_with(b"x"));
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_timeout_with_partial_and_continue() {
    // Time out while reading, capture partial data, then continue
    // reading with a longer timeout to get the rest.
    let mut handle = Exec::cmd("sh")
        .args(&["-c", "printf first; sleep 0.5; printf second"])
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .start()
        .unwrap();

    let mut comm = handle
        .communicate()
        .unwrap()
        .limit_time(Duration::from_millis(100));
    let mut out = vec![];
    let mut err = vec![];
    let result = comm.read_to(&mut out, &mut err);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    // Should have captured "first"
    assert_eq!(out, b"first");

    // Continue reading with longer timeout
    let mut comm = comm.limit_time(Duration::from_secs(2));
    let mut out2 = vec![];
    let mut err2 = vec![];
    comm.read_to(&mut out2, &mut err2).unwrap();
    // Should get "second"
    assert_eq!(out2, b"second");

    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_input_data_flows_through() {
    // Verify that stdin data set via the Exec builder is correctly fed
    // through and appears in the captured output.
    let mut handle = Exec::cmd("cat")
        .stdin("data through builder")
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, _) = handle.communicate().unwrap().read().unwrap();
    assert_eq!(out, b"data through builder");
    assert!(handle.wait().unwrap().success());
}

#[test]
fn communicate_large_input() {
    let input = std::io::repeat(b'x').take(5_000_000);
    let mut handle = Exec::cmd("cat")
        .stdin(InputData::from_reader(input))
        .stdout(Redirection::Pipe)
        .start()
        .unwrap();
    let (out, _) = handle.communicate().unwrap().read().unwrap();
    assert_eq!(out.len(), 5_000_000);
    assert!(out.iter().all(|&b| b == b'x'));
    assert!(handle.wait().unwrap().success());
}
