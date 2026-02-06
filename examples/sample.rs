//! Basic example: list files with line numbers.
//!
//! Run with: cargo run --example sample

use std::io::{BufRead, BufReader};
use subprocess::Exec;

fn main() -> std::io::Result<()> {
    let stream = Exec::cmd("ls").stream_stdout()?;
    let reader = BufReader::new(stream);

    for (i, line) in reader.lines().enumerate() {
        println!("{}: {}", i, line.unwrap());
    }

    Ok(())
}
