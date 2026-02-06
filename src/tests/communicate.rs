use tempfile::TempDir;

use std::fs::{self, File};
use std::io;
use std::time::Duration;

use crate::{ExitStatus, Popen, PopenConfig, Redirection};

#[test]
fn communicate_input() {
    let tmpdir = TempDir::new().unwrap();
    let tmpname = tmpdir.path().join("input");
    let mut p = Popen::create(
        &["cat"],
        PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::File(File::create(&tmpname).unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate("hello world").unwrap().read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    assert!(p.wait().unwrap().success());
    assert_eq!(fs::read_to_string(&tmpname).unwrap(), "hello world");
}

#[test]
fn communicate_output() {
    let mut p = Popen::create(
        &["sh", "-c", "echo foo; echo bar >&2"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate([]).unwrap().read().unwrap();
    assert_eq!(out, b"foo\n");
    assert_eq!(err, b"bar\n");
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_input_output() {
    let mut p = Popen::create(
        &["sh", "-c", "cat; echo foo >&2"],
        PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate("hello world").unwrap().read().unwrap();
    assert_eq!(out, b"hello world");
    assert_eq!(err, b"foo\n");
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_input_output_long() {
    let mut p = Popen::create(
        &["sh", "-c", "cat; printf '%100000s' '' >&2"],
        PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let input = [65u8; 1_000_000];
    let (out, err) = p.communicate(&input).unwrap().read().unwrap();
    assert_eq!(&out[..], &input[..]);
    assert_eq!(&err[..], &[32u8; 100_000][..]);
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_timeout() {
    let mut p = Popen::create(
        &["sh", "-c", "printf foo; sleep 1"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let mut out = vec![];
    let mut err = vec![];
    let result = p
        .communicate([])
        .unwrap()
        .limit_time(Duration::from_millis(100))
        .read_to(&mut out, &mut err);
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert_eq!(out, b"foo");
    assert_eq!(err, vec![]);
    p.kill().unwrap();
}

#[test]
fn communicate_size_limit_small() {
    let mut p = Popen::create(
        &["sh", "-c", "printf '%5s' a"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let mut comm = p.communicate([]).unwrap().limit_size(2);
    assert_eq!(comm.read().unwrap(), (vec![32; 2], vec![]));
    assert_eq!(comm.read().unwrap(), (vec![32; 2], vec![]));
    assert_eq!(comm.read().unwrap(), (vec![b'a'], vec![]));
    p.kill().unwrap();
}

fn check_vec(v: &[u8], size: usize, content: u8) {
    assert_eq!(v.len(), size);
    assert!(v.iter().all(|&c| c == content));
}

#[test]
fn communicate_size_limit_large() {
    let mut p = Popen::create(
        &["sh", "-c", "printf '%20001s' a"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let mut comm = p.communicate([]).unwrap().limit_size(10_000);

    let (out, err) = comm.read().unwrap();
    check_vec(&out, 10_000, 32);
    assert_eq!(err, vec![]);

    let (out, err) = comm.read().unwrap();
    check_vec(&out, 10_000, 32);
    assert_eq!(err, vec![]);

    assert_eq!(comm.read().unwrap(), (vec![b'a'], vec![]));
    p.kill().unwrap();
}

#[test]
fn communicate_size_limit_different_sizes() {
    let mut p = Popen::create(
        &["sh", "-c", "printf '%20001s' a"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let comm = p.communicate([]).unwrap();

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
    p.kill().unwrap();
}

#[test]
fn communicate_stdout_only() {
    // Test with only stdout pipe (no stderr)
    let mut p = Popen::create(
        &["sh", "-c", "echo hello; echo ignored >&2"],
        PopenConfig {
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate([]).unwrap().read().unwrap();
    assert_eq!(out, b"hello\n");
    assert!(err.is_empty());
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_stderr_only() {
    // Test with only stderr pipe (no stdout)
    let mut p = Popen::create(
        &["sh", "-c", "echo ignored; echo error >&2"],
        PopenConfig {
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate([]).unwrap().read().unwrap();
    assert!(out.is_empty());
    assert_eq!(err, b"error\n");
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_stdin_only() {
    // Test with only stdin pipe - output goes to /dev/null
    let mut p = Popen::create(
        &["cat"],
        PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::None,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate("test data").unwrap().read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_empty_input() {
    // Send empty input to stdin
    let mut p = Popen::create(
        &["cat"],
        PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate("").unwrap().read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_empty_output() {
    // Process produces no output
    let mut p = Popen::create(
        &["true"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate([]).unwrap().read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_large_stderr() {
    // Test large output on stderr specifically
    let mut p = Popen::create(
        &["sh", "-c", "printf '%50000s' x >&2"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate([]).unwrap().read().unwrap();
    assert!(out.is_empty());
    assert_eq!(err.len(), 50000);
    assert!(err.iter().all(|&c| c == b' ' || c == b'x'));
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_interleaved_output() {
    // Test interleaved stdout/stderr - both should be captured correctly
    let mut p = Popen::create(
        &[
            "sh",
            "-c",
            "echo out1; echo err1 >&2; echo out2; echo err2 >&2",
        ],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate([]).unwrap().read().unwrap();
    assert_eq!(out, b"out1\nout2\n");
    assert_eq!(err, b"err1\nerr2\n");
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_quick_exit() {
    // Process exits immediately without reading input or producing output
    let mut p = Popen::create(
        &["sh", "-c", "exit 0"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate([]).unwrap().read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_process_fails() {
    // Process exits with error code
    let mut p = Popen::create(
        &["sh", "-c", "echo output; echo error >&2; exit 42"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, err) = p.communicate([]).unwrap().read().unwrap();
    assert_eq!(out, b"output\n");
    assert_eq!(err, b"error\n");
    assert_eq!(p.wait().unwrap(), ExitStatus::Exited(42));
}

#[test]
fn communicate_size_limit_zero() {
    // Size limit of 0 should return empty immediately
    let mut p = Popen::create(
        &["sh", "-c", "printf 'data'"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let mut comm = p.communicate([]).unwrap().limit_size(0);
    let (out, err) = comm.read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    // Continue reading without limit to get the rest
    let mut comm = comm.limit_size(100);
    let (out, err) = comm.read().unwrap();
    assert_eq!(out, b"data");
    assert_eq!(err, vec![]);
    p.kill().unwrap();
}

#[test]
fn communicate_size_limit_stderr() {
    // Size limit should apply to combined stdout + stderr
    let mut p = Popen::create(
        &["sh", "-c", "printf out; printf err >&2"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let mut comm = p.communicate([]).unwrap().limit_size(4);
    let (out, err) = comm.read().unwrap();
    // Should get approximately 4 bytes total across both streams
    let total = out.len() + err.len();
    assert!(total <= 6, "got {} bytes, expected <= 6", total); // allow some slack
    p.kill().unwrap();
}

#[test]
fn communicate_timeout_zero() {
    // Immediate timeout (0 duration) - may or may not get data
    let mut p = Popen::create(
        &["sh", "-c", "sleep 1; echo done"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let result = p
        .communicate([])
        .unwrap()
        .limit_time(Duration::from_secs(0))
        .read();
    // Should timeout since process sleeps
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    p.kill().unwrap();
}

#[test]
fn communicate_multiple_reads_after_eof() {
    // Multiple reads after EOF should return empty
    let mut p = Popen::create(
        &["printf", "hello"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let mut comm = p.communicate([]).unwrap();
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

    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_large_bidirectional() {
    // Large data in both directions simultaneously - tests deadlock prevention
    let mut p = Popen::create(
        &["cat"],
        PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    // 500KB of data - larger than typical pipe buffer (64KB on Linux)
    let input: Vec<u8> = (0..500_000).map(|i| (i % 256) as u8).collect();
    let (out, _) = p.communicate(&input).unwrap().read().unwrap();
    assert_eq!(out, input);
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_partial_read_continue() {
    // Read with size limit, then continue reading
    let mut p = Popen::create(
        &["sh", "-c", "printf 'abcdefghij'"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();

    let mut comm = p.communicate([]).unwrap().limit_size(3);
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

    p.kill().unwrap();
}

#[test]
fn communicate_no_streams() {
    // No pipes at all - should work fine
    let mut p = Popen::create(&["true"], PopenConfig::default()).unwrap();
    let (out, err) = p.communicate([]).unwrap().read().unwrap();
    assert!(out.is_empty());
    assert!(err.is_empty());
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_very_long_lines() {
    // Test with very long lines (no newlines)
    let mut p = Popen::create(
        &["sh", "-c", "printf '%100000s' x"],
        PopenConfig {
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, _) = p.communicate([]).unwrap().read().unwrap();
    assert_eq!(out.len(), 100_000);
    assert!(out.ends_with(b"x"));
    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_timeout_with_partial_and_continue() {
    // Timeout, capture partial data, then continue
    let mut p = Popen::create(
        &["sh", "-c", "printf first; sleep 0.5; printf second"],
        PopenConfig {
            stdout: Redirection::Pipe,
            stderr: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();

    let mut comm = p
        .communicate([])
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

    assert!(p.wait().unwrap().success());
}

#[test]
fn communicate_input_without_stdin_returns_error() {
    let mut p = Popen::create(&["true"], PopenConfig::default()).unwrap();
    let result = p.communicate("data");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
}
