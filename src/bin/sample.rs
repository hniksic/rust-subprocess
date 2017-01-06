extern crate subprocess;

use subprocess::{Exec};
use std::io::{BufReader, BufRead};

fn main() {
    let x = Exec::cmd("ls").stream_stdout().unwrap();
    let br = BufReader::new(x);
    for (i, line) in br.lines().enumerate() {
        println!("{}: {}", i, line.unwrap());
    }
}
