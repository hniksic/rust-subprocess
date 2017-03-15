extern crate tempdir;

use std::ffi::OsString;

use super::super::{Popen, PopenConfig, ExitStatus, Redirection};
use super::super::unix::PopenExt;

use libc;

#[test]
fn err_terminate() {
    let mut p = Popen::create(&["sleep", "5"], PopenConfig::default()).unwrap();
    assert!(p.poll().is_none());
    p.terminate().unwrap();
    assert_eq!(p.wait().unwrap(), ExitStatus::Signaled(libc::SIGTERM as u8));
}

#[test]
fn waitpid_echild() {
    let mut p = Popen::create(&["true"], PopenConfig::default())
        .unwrap();
    let pid = p.pid().unwrap() as i32;
    let mut status = 0 as libc::c_int;
    let wpid = unsafe { libc::waitpid(pid, &mut status, 0) };
    assert_eq!(wpid, pid);
    assert_eq!(status, 0);
    assert_eq!(p.wait().unwrap(), ExitStatus::Undetermined);
}

#[test]
fn send_signal() {
    let mut p = Popen::create(&["sleep", "5"], PopenConfig::default()).unwrap();
    p.send_signal(libc::SIGUSR1).unwrap();
    assert_eq!(p.wait().unwrap(), ExitStatus::Signaled(libc::SIGUSR1 as u8));
}
