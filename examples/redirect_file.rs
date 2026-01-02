//! Redirect subprocess I/O to files.
//!
//! Run with: cargo run --example redirect_file

use std::fs::{self, File};
use std::io::Read;
use subprocess::{Exec, NullFile, Redirection};

fn main() -> subprocess::Result<()> {
    let output_path = "/tmp/subprocess_example_output.txt";
    let input_path = "/tmp/subprocess_example_input.txt";

    // Write stdout to a file
    let output_file = File::create(output_path).unwrap();
    Exec::cmd("echo")
        .arg("This goes to a file")
        .stdout(output_file)
        .join()?;

    let mut contents = String::new();
    File::open(output_path)
        .unwrap()
        .read_to_string(&mut contents)
        .unwrap();
    println!("File contents: {}", contents.trim());

    // Read stdin from a file
    fs::write(input_path, "file contents\n").unwrap();
    let input_file = File::open(input_path).unwrap();
    let output = Exec::cmd("cat")
        .stdin(input_file)
        .stdout(Redirection::Pipe)
        .capture()?
        .stdout_str();
    println!("Read from file: {}", output.trim());

    // Redirect to /dev/null
    Exec::cmd("echo")
        .arg("This output is discarded")
        .stdout(NullFile)
        .join()?;
    println!("Output sent to /dev/null");

    // Redirect stderr to a file, capture stdout
    let stderr_file = File::create(output_path).unwrap();
    let output = Exec::shell("echo stdout; echo stderr >&2")
        .stdout(Redirection::Pipe)
        .stderr(stderr_file)
        .capture()?;

    println!("Captured stdout: {}", output.stdout_str().trim());

    let mut stderr_contents = String::new();
    File::open(output_path)
        .unwrap()
        .read_to_string(&mut stderr_contents)
        .unwrap();
    println!("File has stderr: {}", stderr_contents.trim());

    // Cleanup
    fs::remove_file(output_path).ok();
    fs::remove_file(input_path).ok();

    Ok(())
}
