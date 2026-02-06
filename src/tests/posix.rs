use std::ffi::{OsStr, OsString};

use crate::ExitStatus;
use crate::unix::PopenExt;
use crate::{Popen, PopenConfig, Redirection};

#[test]
fn setup_executable() {
    // Test that PopenConfig::executable overrides the actual executable while argv[0] is
    // passed to the process. We run sh with executable override, and have it print $0
    // which should be "foobar", not "sh".
    let mut p = Popen::create(
        &["foobar", "-c", r#"printf %s "$0""#],
        PopenConfig {
            executable: Some(OsStr::new("sh").to_owned()),
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, _err) = p.communicate([]).unwrap().read_string().unwrap();
    assert_eq!(out, "foobar");
}

#[test]
fn err_terminate() {
    let mut p = Popen::create(&["sleep", "5"], PopenConfig::default()).unwrap();
    assert!(p.poll().is_none());
    p.terminate().unwrap();
    assert!(p.wait().unwrap().is_killed_by(libc::SIGTERM));
}

#[test]
fn waitpid_echild() {
    let mut p = Popen::create(&["true"], PopenConfig::default()).unwrap();
    let pid = p.pid().unwrap() as i32;
    let mut status = 0 as libc::c_int;
    let wpid = unsafe { libc::waitpid(pid, &mut status, 0) };
    assert_eq!(wpid, pid);
    assert_eq!(status, 0);
    let exit = p.wait().unwrap();
    assert!(exit.code().is_none() && exit.signal().is_none());
}

#[test]
fn send_signal() {
    let mut p = Popen::create(&["sleep", "5"], PopenConfig::default()).unwrap();
    p.send_signal(libc::SIGUSR1).unwrap();
    assert_eq!(p.wait().unwrap().signal(), Some(libc::SIGUSR1));
}

#[test]
fn env_set_all_1() {
    let mut p = Popen::create(
        &["env"],
        PopenConfig {
            stdout: Redirection::Pipe,
            env: Some(Vec::new()),
            ..Default::default()
        },
    )
    .unwrap();
    let (out, _err) = p.communicate([]).unwrap().read_string().unwrap();
    assert_eq!(out, "");
}

#[test]
fn env_set_all_2() {
    let mut p = Popen::create(
        &["env"],
        PopenConfig {
            stdout: Redirection::Pipe,
            env: Some(vec![(OsString::from("FOO"), OsString::from("bar"))]),
            ..Default::default()
        },
    )
    .unwrap();
    let (out, _err) = p.communicate([]).unwrap().read_string().unwrap();
    assert_eq!(out.trim_end(), "FOO=bar");
}

#[test]
fn send_signal_group() {
    // Spawn a shell in a new process group that spawns a background child. Signaling the
    // group should terminate both the shell and its child.
    let mut p = Popen::create(
        &["sh", "-c", "sleep 100 & wait"],
        PopenConfig {
            setpgid: true,
            ..Default::default()
        },
    )
    .unwrap();
    p.send_signal_group(libc::SIGTERM).unwrap();
    assert!(p.wait().unwrap().is_killed_by(libc::SIGTERM));
}

#[test]
fn send_signal_group_after_finish() {
    // Signaling a finished process group should succeed (no-op).
    let mut p = Popen::create(
        &["true"],
        PopenConfig {
            setpgid: true,
            ..Default::default()
        },
    )
    .unwrap();
    p.wait().unwrap();
    p.send_signal_group(libc::SIGTERM).unwrap();
}

#[test]
fn kill_process() {
    // kill() sends SIGKILL which cannot be caught
    let mut p = Popen::create(&["sleep", "1000"], PopenConfig::default()).unwrap();
    p.kill().unwrap();
    assert!(p.wait().unwrap().is_killed_by(libc::SIGKILL));
}

#[test]
fn kill_vs_terminate() {
    // Demonstrate that terminate (SIGTERM) and kill (SIGKILL) produce different exit
    // statuses
    let mut p1 = Popen::create(&["sleep", "1000"], PopenConfig::default()).unwrap();
    p1.terminate().unwrap();
    let status1 = p1.wait().unwrap();

    let mut p2 = Popen::create(&["sleep", "1000"], PopenConfig::default()).unwrap();
    p2.kill().unwrap();
    let status2 = p2.wait().unwrap();

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
