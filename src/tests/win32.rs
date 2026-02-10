use crate::{Exec, ExitStatus, Redirection};

#[test]
fn err_terminate() {
    let job = Exec::cmd("sleep").arg("5").start().unwrap();
    assert!(job.poll().is_none());
    job.terminate().unwrap();
    assert_eq!(job.wait().unwrap().code(), Some(1));
}

#[test]
fn exit_status_code() {
    assert_eq!(ExitStatus::from_raw(0).code(), Some(0));
    assert_eq!(ExitStatus::from_raw(1).code(), Some(1));
    assert_eq!(ExitStatus::from_raw(42).code(), Some(42));
}

#[test]
fn exit_status_display() {
    assert_eq!(ExitStatus::from_raw(0).to_string(), "exit code 0");
    assert_eq!(ExitStatus::from_raw(1).to_string(), "exit code 1");
    assert_eq!(ExitStatus::from_raw(42).to_string(), "exit code 42");
}
