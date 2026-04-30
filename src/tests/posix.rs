use std::time::{Duration, Instant};

use super::exec_signal_delay;
use crate::unix::{ExitStatusExt, JobExt, PipelineExt};
use crate::{Exec, ExecExt, ExitStatus, Redirection};

// Tests that close fds 0/1/2 in the parent and let `File::create` reclaim the
// freed slot run their body in a fresh single-test child process. While the
// parent has fd 0/1/2 pointed at a tempfile, any sibling thread writing to
// that fd (notably libtest's "test ... ok" status line) would leak into the
// file and corrupt the test's assertions. Re-execing isolates each such test
// from the rest of the run.
//
// The env var name embeds the parent's PID. The child recognizes itself by
// looking up SUBPROCESS_ISOLATED_FD_TEST_<getppid()> - a stray variable the
// user happens to have set can't match a PID assigned to a not-yet-running
// `cargo test`, so a generic environment leak can't silently disable the
// isolation.
const ISOLATED_TEST_PREFIX: &str = "SUBPROCESS_ISOLATED_FD_TEST_";

fn run_isolated(name: &str, body: impl FnOnce()) {
    let parent_var = format!("{ISOLATED_TEST_PREFIX}{}", unsafe { libc::getppid() });
    if std::env::var_os(&parent_var).is_some() {
        body();
        return;
    }
    let exe = std::env::current_exe().expect("current_exe");
    let test_path = format!("tests::posix::{name}");
    let child_var = format!("{ISOLATED_TEST_PREFIX}{}", std::process::id());
    let output = std::process::Command::new(&exe)
        .args(["--exact", &test_path])
        .env(&child_var, "1")
        .output()
        .expect("spawning isolated test child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "isolated child for {test_path} failed: {status}\n\
         --- child stdout ---\n{stdout}\
         --- child stderr ---\n{stderr}",
        status = output.status,
    );
    // Defense against `name` not matching the surrounding fn name: libtest
    // exits 0 when --exact matches no tests, so the call would silently pass
    // without running the body. "1 passed" only appears when exactly one
    // matching test ran successfully.
    assert!(
        stdout.contains("1 passed"),
        "isolated child for {test_path} matched no tests (typo in name?):\n\
         --- child stdout ---\n{stdout}",
    );
}

#[test]
fn err_terminate() {
    let job = Exec::cmd("sleep").arg("5").start().unwrap();
    exec_signal_delay();
    assert!(job.poll().is_none());
    job.terminate().unwrap();
    assert!(job.wait().unwrap().is_killed_by(libc::SIGTERM));
}

#[test]
fn waitpid_echild() {
    // Start a short-lived process and steal its child with raw waitpid
    // before our Process::wait() gets to it. The library should handle
    // the ECHILD error gracefully.
    let job = Exec::cmd("true").start().unwrap();
    let pid = job.pid() as i32;
    let mut status = 0 as libc::c_int;
    let wpid = unsafe { libc::waitpid(pid, &mut status, 0) };
    assert_eq!(wpid, pid);
    assert_eq!(status, 0);
    let exit = job.wait().unwrap();
    assert!(exit.code().is_none() && exit.signal().is_none());
}

#[test]
fn send_signal() {
    let job = Exec::cmd("sleep").arg("5").start().unwrap();
    exec_signal_delay();
    job.send_signal(libc::SIGUSR1).unwrap();
    assert_eq!(job.wait().unwrap().signal(), Some(libc::SIGUSR1));
}

#[test]
fn env_set_all_1() {
    // An empty environment should result in no env vars being printed.
    let out = Exec::cmd("env")
        .env_clear()
        .stdout(Redirection::Pipe)
        .capture()
        .unwrap()
        .stdout_str();
    assert_eq!(out, "");
}

#[test]
fn env_set_all_2() {
    // A single env var in a cleared environment should be the only
    // output.
    let out = Exec::cmd("env")
        .env_clear()
        .env("FOO", "bar")
        .stdout(Redirection::Pipe)
        .capture()
        .unwrap()
        .stdout_str();
    assert_eq!(out.trim_end(), "FOO=bar");
}

#[test]
fn exec_setpgid() {
    // Spawn a shell in a new process group that spawns a background
    // child. Signaling the group should terminate both the shell and
    // its child.
    let job = Exec::cmd("sh")
        .args(["-c", "sleep 10 & wait"])
        .setpgid()
        .start()
        .unwrap();
    exec_signal_delay();
    job.send_signal_group(libc::SIGTERM).unwrap();
    assert!(job.wait().unwrap().is_killed_by(libc::SIGTERM));
}

#[test]
fn send_signal_group() {
    // Spawn a shell in a new process group that spawns a background
    // child. Signaling the group should terminate both the shell and
    // its child.
    let job = Exec::cmd("sh")
        .args(["-c", "sleep 10 & wait"])
        .setpgid()
        .start()
        .unwrap();
    exec_signal_delay();
    job.send_signal_group(libc::SIGTERM).unwrap();
    assert!(job.wait().unwrap().is_killed_by(libc::SIGTERM));
}

#[test]
fn send_signal_group_after_finish() {
    // Signaling a finished process group should succeed (no-op).
    let job = Exec::cmd("true").setpgid().start().unwrap();
    job.wait().unwrap();
    job.send_signal_group(libc::SIGTERM).unwrap();
}

#[test]
fn kill_process() {
    // kill() sends SIGKILL which cannot be caught.
    let job = Exec::cmd("sleep").arg("10").start().unwrap();
    exec_signal_delay();
    job.kill().unwrap();
    assert!(job.wait().unwrap().is_killed_by(libc::SIGKILL));
}

#[test]
fn kill_vs_terminate() {
    // Demonstrate that terminate (SIGTERM) and kill (SIGKILL) produce
    // different exit statuses.
    let j1 = Exec::cmd("sleep").arg("10").start().unwrap();
    exec_signal_delay();
    j1.terminate().unwrap();
    let status1 = j1.wait().unwrap();

    let j2 = Exec::cmd("sleep").arg("10").start().unwrap();
    exec_signal_delay();
    j2.kill().unwrap();
    let status2 = j2.wait().unwrap();

    assert!(status1.is_killed_by(libc::SIGTERM));
    assert!(status2.is_killed_by(libc::SIGKILL));
    assert_ne!(status1, status2);
}

#[test]
fn exit_status_code() {
    // Unix wait status encoding: exit code is in bits 15..8
    assert_eq!(ExitStatus::from_raw(0 << 8).code(), Some(0));
    assert_eq!(ExitStatus::from_raw(1 << 8).code(), Some(1));
    assert_eq!(ExitStatus::from_raw(42 << 8).code(), Some(42));
    // Signal death: code() returns None
    assert_eq!(ExitStatus::from_raw(9).code(), None); // SIGKILL
}

#[test]
fn exit_status_signal() {
    // Signal death: signal in low 7 bits
    assert_eq!(ExitStatus::from_raw(9).signal(), Some(9)); // SIGKILL
    assert_eq!(
        ExitStatus::from_raw(libc::SIGTERM).signal(),
        Some(libc::SIGTERM)
    );
    // Normal exit: signal() returns None
    assert_eq!(ExitStatus::from_raw(0 << 8).signal(), None);
    assert_eq!(ExitStatus::from_raw(1 << 8).signal(), None);
}

#[test]
fn exit_status_display() {
    assert_eq!(ExitStatus::from_raw(0 << 8).to_string(), "exit code 0");
    assert_eq!(ExitStatus::from_raw(1 << 8).to_string(), "exit code 1");
    assert_eq!(ExitStatus::from_raw(9).to_string(), "signal 9");
}

// --- ExitStatusExt tests ---

#[test]
fn exit_status_ext_round_trip() {
    let status = <ExitStatus as ExitStatusExt>::from_raw(42 << 8);
    assert_eq!(status.into_raw(), Some(42 << 8));
}

// --- pre_exec tests ---

#[test]
fn pre_exec_runs() {
    // pre_exec calls _exit(42) directly; the child never reaches exec, and the parent
    // observes the exit code.
    let job = unsafe {
        Exec::cmd("true")
            .pre_exec(|| libc::_exit(42))
            .start()
            .unwrap()
    };
    let status = job.wait().unwrap();
    assert_eq!(status.code(), Some(42));
}

#[test]
fn pre_exec_error_reported() {
    // A pre_exec closure that returns an error should cause start() to fail.
    let result = unsafe {
        Exec::cmd("true")
            .pre_exec(|| Err(std::io::Error::from_raw_os_error(libc::EACCES)))
            .start()
    };
    let err = result.unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::EACCES));
}

#[test]
fn pre_exec_multiple() {
    // Each closure writes a distinct byte to a pipe the parent holds open; the parent
    // reads back the bytes to verify both closures ran in registration order.
    use std::io::Read;
    use std::os::fd::AsRawFd;
    let (mut read_end, write_end) = crate::posix::pipe().unwrap();
    let fd = write_end.as_raw_fd();
    let job = unsafe {
        Exec::cmd("true")
            .pre_exec(move || {
                let n = libc::write(fd, b"1".as_ptr().cast(), 1);
                if n != 1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            })
            .pre_exec(move || {
                let n = libc::write(fd, b"2".as_ptr().cast(), 1);
                if n != 1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            })
            .start()
            .unwrap()
    };
    drop(write_end);
    let mut buf = [0u8; 2];
    read_end.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"12");
    job.wait().unwrap();
}

// --- arg0 tests ---

#[test]
fn arg0_override() {
    let out = Exec::cmd("sh")
        .arg0("custom-name")
        .args(["-c", "echo $0"])
        .capture()
        .unwrap()
        .stdout_str();
    assert_eq!(out.trim(), "custom-name");
}

// --- JobExt tests ---

#[test]
fn started_send_signal() {
    let job = Exec::cmd("sleep").arg("5").start().unwrap();
    exec_signal_delay();
    job.send_signal(libc::SIGTERM).unwrap();
    let status = job.wait().unwrap();
    assert!(status.is_killed_by(libc::SIGTERM));
}

#[test]
fn started_send_signal_group() {
    let job = Exec::cmd("sh")
        .args(["-c", "sleep 10 & wait"])
        .setpgid()
        .start()
        .unwrap();
    exec_signal_delay();
    job.send_signal_group(libc::SIGKILL).unwrap();
    let status = job.wait().unwrap();
    assert!(status.is_killed_by(libc::SIGKILL) || status.is_killed_by(libc::SIGTERM));
}

// --- Pipeline setpgid tests ---

#[test]
fn pipeline_setpgid() {
    // Spawn a pipeline with setpgid, signal the group, verify all
    // processes die.
    let handle = (Exec::cmd("sleep").arg("5") | Exec::cmd("sleep").arg("5"))
        .setpgid()
        .start()
        .unwrap();
    assert_eq!(handle.processes.len(), 2);
    exec_signal_delay();
    handle.send_signal_group(libc::SIGTERM).unwrap();
    for p in &handle.processes {
        let status = p.wait().unwrap();
        assert!(status.is_killed_by(libc::SIGTERM));
    }
}

#[test]
fn pipeline_setpgid_rejects_exec_setpgid() {
    // Using Exec::setpgid() inside a pipeline should return an error.
    let result = (Exec::cmd("true").setpgid() | Exec::cmd("true")).start();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(err.to_string().contains("setpgid"));
}

#[test]
fn user_file_at_target_fd_survives_exec() {
    // A File passed as redirection whose raw fd already equals the target
    // stream fd must remain open in the child after exec. Set up by closing
    // fd 0 in the parent and opening a file so it lands on fd 0.
    run_isolated("user_file_at_target_fd_survives_exec", || {
        use std::fs::File;
        use std::os::fd::AsRawFd;
        use tempfile::TempDir;

        let tmpdir = TempDir::new().unwrap();
        let tmpname = tmpdir.path().join("input");
        std::fs::write(&tmpname, "stdin-payload").unwrap();

        let saved = unsafe { libc::dup(0) };
        assert!(saved >= 0);
        let close_rc = unsafe { libc::close(0) };
        assert_eq!(close_rc, 0);
        let f = File::open(&tmpname).unwrap();
        assert_eq!(f.as_raw_fd(), 0, "test setup: file did not land at fd 0");
        // Park the parent's original stdin at fd 100 until we restore it.
        let dup_rc = unsafe { libc::dup2(saved, 100) };
        assert!(dup_rc >= 0);
        unsafe {
            libc::close(saved);
        }

        let result = Exec::cmd("cat")
            .stdin(f)
            .stdout(Redirection::Pipe)
            .stderr(Redirection::Pipe)
            .capture();

        // Restore the parent's stdin.
        unsafe {
            libc::dup2(100, 0);
            libc::close(100);
        }

        let c = result.expect("capture failed");
        assert_eq!(
            c.stdout_str(),
            "stdin-payload",
            "stderr was: {:?}",
            c.stderr_str()
        );
        assert!(c.exit_status.success());
    });
}

#[test]
fn user_file_at_other_standard_fd_preserves_inherited_stream() {
    // A File passed as redirection whose raw fd is a standard fd *other than*
    // its target_fd (e.g., file at fd 2 used as stdout) must not have that
    // standard fd closed by install_child_fd. Otherwise the child loses its
    // inherited standard stream that was at that slot.
    //
    // Set up by closing fd 2 and opening a file so it lands on fd 2, then use
    // it as stdout. fd 2 must still be open in the child when pre_exec runs.
    run_isolated(
        "user_file_at_other_standard_fd_preserves_inherited_stream",
        || {
            use std::fs::File;
            use std::io::Read;
            use std::os::fd::AsRawFd;
            use tempfile::TempDir;

            let tmpdir = TempDir::new().unwrap();
            let tmpname = tmpdir.path().join("output");

            let saved = unsafe { libc::dup(2) };
            assert!(saved >= 0);
            let close_rc = unsafe { libc::close(2) };
            assert_eq!(close_rc, 0);
            let f = File::create(&tmpname).unwrap();
            assert_eq!(f.as_raw_fd(), 2, "test setup: file did not land at fd 2");
            // Park the parent's original stderr at fd 100 until we restore it.
            let dup_rc = unsafe { libc::dup2(saved, 100) };
            assert!(dup_rc >= 0);
            unsafe {
                libc::close(saved);
            }

            // Pipe to receive the child's report on whether fd 2 is still open
            // after install_child_fd has run. The pipe ends are CLOEXEC, so the
            // write end closes at exec without us needing to set anything up
            // here.
            let (mut read_end, write_end) = crate::posix::pipe().unwrap();
            let report_fd = write_end.as_raw_fd();

            let result = unsafe {
                Exec::cmd("true")
                    .stdout(f)
                    .pre_exec(move || {
                        let r = libc::fcntl(2, libc::F_GETFD);
                        let msg: &[u8] = if r >= 0 { b"open" } else { b"clsd" };
                        libc::write(report_fd, msg.as_ptr().cast(), msg.len());
                        Ok(())
                    })
                    .start()
            };

            // Restore the parent's stderr.
            unsafe {
                libc::dup2(100, 2);
                libc::close(100);
            }
            drop(write_end);

            let job = result.expect("start failed");
            let mut buf = [0u8; 4];
            read_end.read_exact(&mut buf).unwrap();
            let _ = job.wait();
            assert_eq!(
                &buf, b"open",
                "fd 2 was closed in the child by install_child_fd"
            );
        },
    );
}

#[test]
fn stdin_pipe_with_user_stdout_at_fd_0() {
    // A user-supplied File whose raw fd is 0, used as stdout, must not be
    // clobbered by the install_child_fd call that places stdin onto fd 0.
    // redirect_streams must install stdout (which dup2s from fd 0) before
    // stdin (which overwrites fd 0).
    run_isolated("stdin_pipe_with_user_stdout_at_fd_0", || {
        use std::fs::File;
        use std::os::fd::AsRawFd;
        use tempfile::TempDir;

        let tmpdir = TempDir::new().unwrap();
        let outfile = tmpdir.path().join("output");

        let saved = unsafe { libc::dup(0) };
        assert!(saved >= 0);
        assert_eq!(unsafe { libc::close(0) }, 0);
        let f = File::create(&outfile).unwrap();
        assert_eq!(f.as_raw_fd(), 0, "test setup: file did not land at fd 0");
        assert!(unsafe { libc::dup2(saved, 100) } >= 0);
        unsafe {
            libc::close(saved);
        }

        let result = Exec::cmd("printf")
            .args(["%s", "hello"])
            .stdin(Redirection::Pipe)
            .stdout(f)
            .stderr(Redirection::Pipe)
            .capture();

        unsafe {
            libc::dup2(100, 0);
            libc::close(100);
        }

        let c = result.expect("capture failed");
        assert!(
            c.exit_status.success(),
            "printf failed; stderr: {:?}",
            c.stderr_str()
        );
        let content = std::fs::read_to_string(&outfile).unwrap();
        assert_eq!(content, "hello");
    });
}

#[test]
fn stdout_pipe_with_user_stderr_at_fd_1() {
    // A user-supplied File whose raw fd is 1, used as stderr, must not be
    // clobbered by the install_child_fd call that places stdout onto fd 1.
    // redirect_streams must install stderr (which dup2s from fd 1) before
    // stdout (which overwrites fd 1).
    run_isolated("stdout_pipe_with_user_stderr_at_fd_1", || {
        use std::fs::File;
        use std::os::fd::AsRawFd;
        use tempfile::TempDir;

        let tmpdir = TempDir::new().unwrap();
        let errfile = tmpdir.path().join("err");

        let saved = unsafe { libc::dup(1) };
        assert!(saved >= 0);
        assert_eq!(unsafe { libc::close(1) }, 0);
        let f = File::create(&errfile).unwrap();
        assert_eq!(f.as_raw_fd(), 1, "test setup: file did not land at fd 1");
        assert!(unsafe { libc::dup2(saved, 100) } >= 0);
        unsafe {
            libc::close(saved);
        }

        let result = Exec::cmd("sh")
            .args(["-c", "echo to-stdout; echo to-stderr >&2"])
            .stdout(Redirection::Pipe)
            .stderr(f)
            .capture();

        unsafe {
            libc::dup2(100, 1);
            libc::close(100);
        }

        let c = result.expect("capture failed");
        assert!(c.exit_status.success());
        assert_eq!(c.stdout_str().trim(), "to-stdout");
        let stderr_content = std::fs::read_to_string(&errfile).unwrap();
        assert_eq!(stderr_content.trim(), "to-stderr");
    });
}

#[test]
fn stdin_pipe_with_user_stderr_at_fd_0() {
    // A user-supplied File whose raw fd is 0, used as stderr, must not be
    // clobbered by the install_child_fd call that places stdin onto fd 0.
    run_isolated("stdin_pipe_with_user_stderr_at_fd_0", || {
        use std::fs::File;
        use std::os::fd::AsRawFd;
        use tempfile::TempDir;

        let tmpdir = TempDir::new().unwrap();
        let errfile = tmpdir.path().join("err");

        let saved = unsafe { libc::dup(0) };
        assert!(saved >= 0);
        assert_eq!(unsafe { libc::close(0) }, 0);
        let f = File::create(&errfile).unwrap();
        assert_eq!(f.as_raw_fd(), 0, "test setup: file did not land at fd 0");
        assert!(unsafe { libc::dup2(saved, 100) } >= 0);
        unsafe {
            libc::close(saved);
        }

        let result = Exec::cmd("sh")
            .args(["-c", "echo to-stderr >&2"])
            .stdin(Redirection::Pipe)
            .stdout(Redirection::Pipe)
            .stderr(f)
            .capture();

        unsafe {
            libc::dup2(100, 0);
            libc::close(100);
        }

        let c = result.expect("capture failed");
        assert!(c.exit_status.success());
        let stderr_content = std::fs::read_to_string(&errfile).unwrap();
        assert_eq!(stderr_content.trim(), "to-stderr");
    });
}

#[test]
fn user_files_with_swapped_fds_resolve_cycle() {
    // The cyclic case: stdout's source fd is stderr's target, and stderr's
    // source fd is stdout's target. No reorder alone can install both
    // correctly; redirect_streams must dup one source via F_DUPFD_CLOEXEC to
    // break the cycle.
    run_isolated("user_files_with_swapped_fds_resolve_cycle", || {
        use std::fs::File;
        use std::os::fd::AsRawFd;
        use tempfile::TempDir;

        let tmpdir = TempDir::new().unwrap();
        let path_at_1 = tmpdir.path().join("at_fd_1");
        let path_at_2 = tmpdir.path().join("at_fd_2");

        let saved_1 = unsafe { libc::dup(1) };
        let saved_2 = unsafe { libc::dup(2) };
        assert!(saved_1 >= 0 && saved_2 >= 0);
        assert_eq!(unsafe { libc::close(1) }, 0);
        assert_eq!(unsafe { libc::close(2) }, 0);

        let file_at_1 = File::create(&path_at_1).unwrap();
        assert_eq!(file_at_1.as_raw_fd(), 1);
        let file_at_2 = File::create(&path_at_2).unwrap();
        assert_eq!(file_at_2.as_raw_fd(), 2);

        assert!(unsafe { libc::dup2(saved_1, 100) } >= 0);
        assert!(unsafe { libc::dup2(saved_2, 101) } >= 0);
        unsafe {
            libc::close(saved_1);
            libc::close(saved_2);
        }

        // file_at_2 (fd=2) used as stdout, file_at_1 (fd=1) used as stderr -> cycle.
        let result = Exec::cmd("sh")
            .args(["-c", "echo out; echo err >&2"])
            .stdout(file_at_2)
            .stderr(file_at_1)
            .join();

        unsafe {
            libc::dup2(100, 1);
            libc::dup2(101, 2);
            libc::close(100);
            libc::close(101);
        }

        let status = result.expect("spawn failed");
        assert!(status.success());

        let content_at_1 = std::fs::read_to_string(&path_at_1).unwrap();
        let content_at_2 = std::fs::read_to_string(&path_at_2).unwrap();
        assert_eq!(
            content_at_1.trim(),
            "err",
            "stderr file should contain 'err'"
        );
        assert_eq!(
            content_at_2.trim(),
            "out",
            "stdout file should contain 'out'"
        );
    });
}

#[cfg(target_os = "linux")]
#[test]
fn pipeline_stderr_all_non_cloexec_file_does_not_leak() {
    // Regression test: a Pipeline with stderr_all(File) shares one Arc across
    // all commands. Non-last commands see Arc::strong_count > 1 in the child,
    // which used to short-circuit prevent_dealloc and leave the source fd open
    // in the child. If the user's File lacks CLOEXEC, the fd survived into the
    // exec'd binary.
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use tempfile::TempDir;

    let tmpdir = TempDir::new().unwrap();
    let errfile = tmpdir.path().join("err");
    let report = tmpdir.path().join("report");

    let f = File::create(&errfile).unwrap();
    let raw = f.as_raw_fd();
    // Clear CLOEXEC so a leaked fd would otherwise survive exec.
    unsafe {
        let flags = libc::fcntl(raw, libc::F_GETFD);
        assert!(flags >= 0);
        let r = libc::fcntl(raw, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
        assert_eq!(r, 0);
    }

    // First (non-last) child checks whether fd `raw` is open in its address
    // space after exec. With the bug present the file is still mapped at fd
    // `raw`; with the fix CLOEXEC has been set so /proc/self/fd/<raw> is gone.
    let check_cmd = format!(
        "if [ -e /proc/self/fd/{} ]; then echo LEAK > {}; \
         else echo CLEAR > {}; fi",
        raw,
        report.display(),
        report.display(),
    );
    let p = Exec::shell(check_cmd) | Exec::cmd("true");
    p.stderr_all(f).join().unwrap();

    let content = std::fs::read_to_string(&report).unwrap();
    assert_eq!(
        content.trim(),
        "CLEAR",
        "user File fd was leaked into a non-last pipeline child"
    );
}

#[test]
fn null_redirect_does_not_leak_fd() {
    // Regression test for issue #81. When bash spawns a background process ("sleep 10
    // &"), it won't return from "wait" until the backgrounded child also closes its
    // inherited file descriptors. If we leak the /dev/null fds to the child, the
    // backgrounded sleep keeps them open and join() hangs.
    let start = Instant::now();
    let status = Exec::cmd("sh")
        .args(["-c", "sleep 10 &"])
        .stdout(Redirection::Null)
        .stderr(Redirection::Null)
        .join()
        .unwrap();
    assert!(status.success());
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "join() took too long, /dev/null fds may have leaked"
    );
}

#[test]
fn poll_does_not_block_during_wait() {
    // Process::poll() is documented as non-blocking. Verify that a poll() in one thread
    // is not serialized behind a blocking wait() in another, even though both touch the
    // shared exit-status state.
    use std::thread;

    let job = Exec::cmd("sleep").arg("5").start().unwrap();
    let process = job.processes[0].clone();

    // Park a thread in wait().
    let waiter_proc = process.clone();
    let waiter = thread::spawn(move || waiter_proc.wait().unwrap());

    // Give the waiter time to actually enter the blocking syscall.
    thread::sleep(Duration::from_millis(100));

    let start = Instant::now();
    let status = process.poll();
    let elapsed = start.elapsed();
    assert!(status.is_none(), "child should still be running");
    assert!(
        elapsed < Duration::from_millis(200),
        "poll() took {:?}, expected to return immediately while wait() blocks",
        elapsed
    );

    // Unblock the waiter so the test doesn't sit out the child's full sleep.
    process.terminate().unwrap();
    let _ = waiter.join().unwrap();
}

#[test]
fn terminate_during_wait() {
    // terminate() from one thread must reach the child while another thread is blocked
    // in wait(), and must not signal a recycled PID after the child has been reaped.
    use std::thread;

    let job = Exec::cmd("sleep").arg("10").start().unwrap();
    let process = job.processes[0].clone();

    let waiter_proc = process.clone();
    let waiter = thread::spawn(move || waiter_proc.wait().unwrap());

    // Let the waiter reach its blocking syscall before we signal.
    thread::sleep(Duration::from_millis(100));

    let start = Instant::now();
    process.terminate().unwrap();
    let term_elapsed = start.elapsed();
    assert!(
        term_elapsed < Duration::from_millis(200),
        "terminate() took {:?}, expected to return immediately while wait() blocks",
        term_elapsed
    );

    let status = waiter.join().unwrap();
    assert!(status.is_killed_by(libc::SIGTERM));
}
