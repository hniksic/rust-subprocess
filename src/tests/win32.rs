use super::super::{ExitStatus, Popt};

#[test]
fn err_terminate() {
    let mut p = Popt::new("sleep").arg("5").spawn().unwrap();
    assert!(p.poll().is_none());
    p.terminate().unwrap();
    assert!(p.wait().unwrap() == ExitStatus::Exited(1));
}
