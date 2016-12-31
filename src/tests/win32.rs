use super::super::{ExitStatus, Run};

#[test]
fn err_terminate() {
    let mut p = Run::new("sleep").arg("5").popen().unwrap();
    assert!(p.poll().is_none());
    p.terminate().unwrap();
    assert!(p.wait().unwrap() == ExitStatus::Exited(1));
}
