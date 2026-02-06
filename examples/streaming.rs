//! Stream subprocess output line by line.
//!
//! Run with: cargo run --example streaming

use std::io::{BufRead, BufReader, Write};
use subprocess::Exec;

fn main() -> std::io::Result<()> {
    // Stream stdout line by line
    println!("Reading output line by line:");
    let stream = Exec::shell("printf 'line 1\nline 2\nline 3\n'").stream_stdout()?;

    for (i, line) in BufReader::new(stream).lines().enumerate() {
        println!("  {}: {}", i + 1, line.unwrap());
    }

    // Stream into stdin
    println!("\nWriting to subprocess stdin:");
    let mut writer = Exec::cmd("cat").stream_stdin()?;

    writeln!(writer, "First line")?;
    writeln!(writer, "Second line")?;
    writer.flush()?;
    drop(writer); // Close stdin to signal EOF

    // Read stderr separately
    println!("\nReading stderr:");
    let stream = Exec::shell("echo 'error message' >&2").stream_stderr()?;

    for line in BufReader::new(stream).lines() {
        println!("  stderr: {}", line.unwrap());
    }

    Ok(())
}
