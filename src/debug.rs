extern crate subprocess;

use subprocess::{Popen, Redirection};

fn main() {
    let mut p = Popen::create_full(&["sh", "-c", "echo foo; echo bar >&2"],
                               Redirection::None, Redirection::None, Redirection::Merge)
        .unwrap();
    p.wait().unwrap();
}
