//! Capture the output of a command.
//!
//! Run with: cargo run --example capture_output

use subprocess::{Exec, Redirection};

fn main() -> std::io::Result<()> {
    // Simple capture of stdout
    let output = Exec::cmd("echo")
        .arg("Hello from subprocess!")
        .stdout(Redirection::Pipe)
        .capture()?;

    println!("Output: {}", output.stdout_str().trim());
    println!("Exit status: {:?}", output.exit_status);

    // Capture both stdout and stderr (merged)
    let output = Exec::shell("echo stdout; echo stderr >&2")
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Merge)
        .capture()?;

    println!("\nMerged output: {}", output.stdout_str().trim());

    // Capture stdout and stderr separately
    let output = Exec::shell("echo out; echo err >&2")
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .capture()?;

    println!("\nSeparate streams:");
    println!("  stdout: {}", output.stdout_str().trim());
    println!("  stderr: {}", output.stderr_str().trim());

    Ok(())
}
