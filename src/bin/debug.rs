extern crate subprocess;

use subprocess::popen;

fn main() {
    let mut p = popen::Popen::create(&["sleep", "5"]).unwrap();
    p.terminate().unwrap();
    assert!(p.wait().unwrap() == popen::ExitStatus::Signaled(popen::SIGTERM));
}
