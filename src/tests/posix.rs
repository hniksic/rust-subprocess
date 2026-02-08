use std::time::{Duration, Instant};

use crate::unix::{JobExt, PipelineExt, ProcessExt};
use crate::{Exec, ExecExt, ExitStatus, Redirection};

#[test]
fn err_terminate() {
    let handle = Exec::cmd("sleep").arg("5").start().unwrap();
    assert!(handle.processes[0].poll().is_none());
    handle.processes[0].terminate().unwrap();
    assert!(
        handle.processes[0]
            .wait()
            .unwrap()
            .is_killed_by(libc::SIGTERM)
    );
}

#[test]
fn waitpid_echild() {
    // Start a short-lived process and steal its child with raw waitpid
    // before our Process::wait() gets to it. The library should handle
    // the ECHILD error gracefully.
    let handle = Exec::cmd("true").start().unwrap();
    let pid = handle.processes[0].pid() as i32;
    let mut status = 0 as libc::c_int;
    let wpid = unsafe { libc::waitpid(pid, &mut status, 0) };
    assert_eq!(wpid, pid);
    assert_eq!(status, 0);
    let exit = handle.processes[0].wait().unwrap();
    assert!(exit.code().is_none() && exit.signal().is_none());
}

#[test]
fn send_signal() {
    let handle = Exec::cmd("sleep").arg("5").start().unwrap();
    handle.processes[0].send_signal(libc::SIGUSR1).unwrap();
    assert_eq!(
        handle.processes[0].wait().unwrap().signal(),
        Some(libc::SIGUSR1)
    );
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
    let handle = Exec::cmd("sh")
        .args(&["-c", "sleep 100 & wait"])
        .setpgid()
        .start()
        .unwrap();
    handle.processes[0]
        .send_signal_group(libc::SIGTERM)
        .unwrap();
    assert!(
        handle.processes[0]
            .wait()
            .unwrap()
            .is_killed_by(libc::SIGTERM)
    );
}

#[test]
fn send_signal_group() {
    // Spawn a shell in a new process group that spawns a background
    // child. Signaling the group should terminate both the shell and
    // its child.
    let handle = Exec::cmd("sh")
        .args(&["-c", "sleep 100 & wait"])
        .setpgid()
        .start()
        .unwrap();
    handle.processes[0]
        .send_signal_group(libc::SIGTERM)
        .unwrap();
    assert!(
        handle.processes[0]
            .wait()
            .unwrap()
            .is_killed_by(libc::SIGTERM)
    );
}

#[test]
fn send_signal_group_after_finish() {
    // Signaling a finished process group should succeed (no-op).
    let handle = Exec::cmd("true").setpgid().start().unwrap();
    handle.processes[0].wait().unwrap();
    handle.processes[0]
        .send_signal_group(libc::SIGTERM)
        .unwrap();
}

#[test]
fn kill_process() {
    // kill() sends SIGKILL which cannot be caught.
    let handle = Exec::cmd("sleep").arg("1000").start().unwrap();
    handle.processes[0].kill().unwrap();
    assert!(
        handle.processes[0]
            .wait()
            .unwrap()
            .is_killed_by(libc::SIGKILL)
    );
}

#[test]
fn kill_vs_terminate() {
    // Demonstrate that terminate (SIGTERM) and kill (SIGKILL) produce
    // different exit statuses.
    let h1 = Exec::cmd("sleep").arg("1000").start().unwrap();
    h1.processes[0].terminate().unwrap();
    let status1 = h1.processes[0].wait().unwrap();

    let h2 = Exec::cmd("sleep").arg("1000").start().unwrap();
    h2.processes[0].kill().unwrap();
    let status2 = h2.processes[0].wait().unwrap();

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

// --- JobExt tests ---

#[test]
fn started_send_signal() {
    let handle = Exec::cmd("sleep").arg("100").start().unwrap();
    handle.send_signal(libc::SIGTERM).unwrap();
    let status = handle.processes[0].wait().unwrap();
    assert!(status.is_killed_by(libc::SIGTERM));
}

#[test]
fn started_send_signal_group() {
    let handle = Exec::cmd("sh")
        .args(&["-c", "sleep 100 & wait"])
        .setpgid()
        .start()
        .unwrap();
    handle.send_signal_group(libc::SIGKILL).unwrap();
    let status = handle.processes[0].wait().unwrap();
    assert!(status.is_killed_by(libc::SIGKILL) || status.is_killed_by(libc::SIGTERM));
}

// --- Pipeline setpgid tests ---

#[test]
fn pipeline_setpgid() {
    // Spawn a pipeline with setpgid, signal the group, verify all
    // processes die.
    let handle = (Exec::cmd("sleep").arg("100") | Exec::cmd("sleep").arg("100"))
        .setpgid()
        .start()
        .unwrap();
    assert_eq!(handle.processes.len(), 2);
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
fn null_redirect_does_not_leak_fd() {
    // Regression test for issue #81. When bash spawns a background
    // process ("sleep 100 &"), it won't return from "wait" until the
    // backgrounded child also closes its inherited file descriptors. If
    // we leak the /dev/null fds to the child, the backgrounded sleep
    // keeps them open and join() hangs.
    let start = Instant::now();
    let status = Exec::cmd("bash")
        .args(&["-c", "sleep 100 &"])
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
