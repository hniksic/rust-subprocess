extern crate tempdir;

use super::super::{Popen, PopenConfig, ExitStatus};
use super::super::posix;

#[test]
fn err_terminate() {
    let mut p = Popen::create(&["sleep", "5"], PopenConfig::default()).unwrap();
    assert!(p.poll().is_none());
    p.terminate().unwrap();
    assert!(p.wait().unwrap() == ExitStatus::Signaled(posix::SIGTERM as u8));
}

#[test]
fn waitpid_echild() {
    let mut p = Popen::create(&["true"], PopenConfig::default())
        .unwrap();
    let pid = p.pid().unwrap();
    let (wpid, status) = posix::waitpid(pid, 0).unwrap();
    assert!(wpid == pid);
    assert!(status == ExitStatus::Exited(0));
    assert!(p.wait().unwrap() == ExitStatus::Undetermined);
}
