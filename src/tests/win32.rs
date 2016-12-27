use super::super::{Popen, ExitStatus};

#[test]
fn err_terminate() {
    let mut p = Popen::create(&["sleep", "5"]).unwrap();
    assert!(p.poll().is_none());
    p.terminate().unwrap();
    assert!(p.wait().unwrap() == ExitStatus::Exited(1));
}
