extern crate subprocess;

use subprocess::{Popen, PopenConfig, Redirection};

fn main() {
    let mut p = Popen::create(&["sh", "-c", "echo foo; echo bar >&2"], PopenConfig {
        stderr: Redirection::Merge, ..Default::default()
    }).unwrap();
    p.wait().unwrap();
}
