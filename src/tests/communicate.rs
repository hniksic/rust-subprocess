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
    assert!(matches!(
        p.communicate_bytes(Some(b"hello world")),
        Ok((None, None)),
    ));
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
    assert!(matches!(
        p.communicate_bytes(None),
        Ok((Some(out), Some(err))) if {
            assert_eq!(out, b"foo\n");
            assert_eq!(err, b"bar\n");
            true
        }
    ));
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
    assert!(matches!(
        p.communicate_bytes(Some(b"hello world")),
        Ok((Some(out), Some(err))) if {
            assert_eq!(out, b"hello world");
            assert_eq!(err, b"foo\n");
            true
        }
    ));
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
    assert!(matches!(
        p.communicate_bytes(Some(&input)),
        Ok((Some(out), Some(err))) if {
            assert_eq!(&out[..], &input[..]);
            assert_eq!(&err[..], &[32u8; 100_000][..]);
            true
        }
    ));
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
    match p
        .communicate_start(None)
        .limit_time(Duration::from_millis(100))
        .read()
    {
        Err(e) => {
            assert_eq!(e.kind(), io::ErrorKind::TimedOut);
            assert_eq!(e.capture, (Some(b"foo".to_vec()), Some(vec![])));
        }
        other => panic!("unexpected result {:?}", other),
    }
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
    let mut comm = p.communicate_start(None).limit_size(2);
    assert_eq!(comm.read().unwrap(), (Some(vec![32; 2]), Some(vec![])));
    assert_eq!(comm.read().unwrap(), (Some(vec![32; 2]), Some(vec![])));
    assert_eq!(comm.read().unwrap(), (Some(vec![b'a']), Some(vec![])));
    p.kill().unwrap();
}

fn check_vec(v: Option<Vec<u8>>, size: usize, content: u8) {
    assert_eq!(v.as_ref().unwrap().len(), size);
    assert!(v.as_ref().unwrap().iter().all(|&c| c == content));
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
    let mut comm = p.communicate_start(None).limit_size(10_000);

    let (out, err) = comm.read().unwrap();
    check_vec(out, 10_000, 32);
    assert_eq!(err, Some(vec![]));

    let (out, err) = comm.read().unwrap();
    check_vec(out, 10_000, 32);
    assert_eq!(err, Some(vec![]));

    assert_eq!(comm.read().unwrap(), (Some(vec![b'a']), Some(vec![])));
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
    let comm = p.communicate_start(None);

    let mut comm = comm.limit_size(100);
    let (out, err) = comm.read().unwrap();
    check_vec(out, 100, 32);
    assert_eq!(err, Some(vec![]));

    let mut comm = comm.limit_size(1_000);
    let (out, err) = comm.read().unwrap();
    check_vec(out, 1_000, 32);
    assert_eq!(err, Some(vec![]));

    let mut comm = comm.limit_size(10_000);
    let (out, err) = comm.read().unwrap();
    check_vec(out, 10_000, 32);
    assert_eq!(err, Some(vec![]));

    let mut comm = comm.limit_size(8_900);
    let (out, err) = comm.read().unwrap();
    check_vec(out, 8_900, 32);
    assert_eq!(err, Some(vec![]));

    assert_eq!(comm.read().unwrap(), (Some(vec![b'a']), Some(vec![])));
    assert_eq!(comm.read().unwrap(), (Some(vec![]), Some(vec![])));
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
    let (out, err) = p.communicate_bytes(None).unwrap();
    assert_eq!(out, Some(b"hello\n".to_vec()));
    assert_eq!(err, None);
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
    let (out, err) = p.communicate_bytes(None).unwrap();
    assert_eq!(out, None);
    assert_eq!(err, Some(b"error\n".to_vec()));
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
    let (out, err) = p.communicate_bytes(Some(b"test data")).unwrap();
    assert_eq!(out, None);
    assert_eq!(err, None);
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
    let (out, err) = p.communicate_bytes(Some(b"")).unwrap();
    assert_eq!(out, Some(vec![]));
    assert_eq!(err, None);
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
    let (out, err) = p.communicate_bytes(None).unwrap();
    assert_eq!(out, Some(vec![]));
    assert_eq!(err, Some(vec![]));
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
    let (out, err) = p.communicate_bytes(None).unwrap();
    assert_eq!(out, Some(vec![]));
    let err = err.unwrap();
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
    let (out, err) = p.communicate_bytes(None).unwrap();
    assert_eq!(out.unwrap(), b"out1\nout2\n");
    assert_eq!(err.unwrap(), b"err1\nerr2\n");
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
    let (out, err) = p.communicate_bytes(None).unwrap();
    assert_eq!(out, Some(vec![]));
    assert_eq!(err, Some(vec![]));
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
    let (out, err) = p.communicate_bytes(None).unwrap();
    assert_eq!(out, Some(b"output\n".to_vec()));
    assert_eq!(err, Some(b"error\n".to_vec()));
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
    let mut comm = p.communicate_start(None).limit_size(0);
    let (out, err) = comm.read().unwrap();
    assert_eq!(out, Some(vec![]));
    assert_eq!(err, Some(vec![]));
    // Continue reading without limit to get the rest
    let mut comm = comm.limit_size(100);
    let (out, err) = comm.read().unwrap();
    assert_eq!(out, Some(b"data".to_vec()));
    assert_eq!(err, Some(vec![]));
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
    let mut comm = p.communicate_start(None).limit_size(4);
    let (out, err) = comm.read().unwrap();
    // Should get approximately 4 bytes total across both streams
    let total = out.as_ref().map_or(0, |v| v.len()) + err.as_ref().map_or(0, |v| v.len());
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
        .communicate_start(None)
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
    let mut comm = p.communicate_start(None);
    let (out, err) = comm.read().unwrap();
    assert_eq!(out, Some(b"hello".to_vec()));
    assert_eq!(err, Some(vec![]));

    // Subsequent reads should return empty
    let (out, err) = comm.read().unwrap();
    assert_eq!(out, Some(vec![]));
    assert_eq!(err, Some(vec![]));

    // And again
    let (out, err) = comm.read().unwrap();
    assert_eq!(out, Some(vec![]));
    assert_eq!(err, Some(vec![]));

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
    let (out, _) = p.communicate_bytes(Some(&input)).unwrap();
    assert_eq!(out.unwrap(), input);
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

    let mut comm = p.communicate_start(None).limit_size(3);
    let (out1, _) = comm.read().unwrap();
    assert_eq!(out1.as_ref().unwrap().len(), 3);

    let mut comm = comm.limit_size(3);
    let (out2, _) = comm.read().unwrap();
    assert_eq!(out2.as_ref().unwrap().len(), 3);

    // Read the rest
    let mut comm = comm.limit_size(100);
    let (out3, _) = comm.read().unwrap();

    // Combine all reads
    let mut combined = out1.unwrap();
    combined.extend(out2.unwrap());
    combined.extend(out3.unwrap());
    assert_eq!(combined, b"abcdefghij");

    p.kill().unwrap();
}

#[test]
fn communicate_no_streams() {
    // No pipes at all - should work fine
    let mut p = Popen::create(&["true"], PopenConfig::default()).unwrap();
    let (out, err) = p.communicate_bytes(None).unwrap();
    assert_eq!(out, None);
    assert_eq!(err, None);
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
    let (out, _) = p.communicate_bytes(None).unwrap();
    let out = out.unwrap();
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
        .communicate_start(None)
        .limit_time(Duration::from_millis(100));
    let result = comm.read();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    // Should have captured "first"
    assert_eq!(err.capture.0, Some(b"first".to_vec()));

    // Continue reading with longer timeout
    let mut comm = comm.limit_time(Duration::from_secs(2));
    let (out, _) = comm.read().unwrap();
    // Should get "second"
    assert_eq!(out, Some(b"second".to_vec()));

    assert!(p.wait().unwrap().success());
}

#[test]
#[should_panic(expected = "must provide input")]
fn communicate_stdin_without_input_panics() {
    let mut p = Popen::create(
        &["cat"],
        PopenConfig {
            stdin: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let _ = p.communicate_bytes(None);
}

#[test]
#[should_panic(expected = "cannot provide input")]
fn communicate_input_without_stdin_panics() {
    let mut p = Popen::create(&["true"], PopenConfig::default()).unwrap();
    let _ = p.communicate_bytes(Some(b"data"));
}
