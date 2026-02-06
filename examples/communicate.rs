//! Bidirectional communication with a subprocess.
//!
//! Run with: cargo run --example communicate

use std::time::Duration;
use subprocess::{Popen, PopenConfig, Redirection};

fn main() -> std::io::Result<()> {
    // Basic communicate: send input, receive output
    let mut p = Popen::create(
        &["cat"],
        PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )?;

    let (stdout, _stderr) = p.communicate("Hello, cat!")?.read_string()?;
    println!("cat said: {}", stdout);
    p.wait()?;

    // Communicate with timeout using Communicator
    println!("\nCommunicating with timeout:");
    let mut p = Popen::create(
        &["cat"],
        PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )?;

    let result = p
        .communicate("data with timeout")?
        .limit_time(Duration::from_secs(5))
        .read();

    match result {
        Ok((stdout, _)) => println!("Got: {}", String::from_utf8_lossy(&stdout)),
        Err(e) => println!("Error: {}", e),
    }
    p.wait()?;

    // Communicate with size limit
    println!("\nCommunicating with size limit:");
    let mut p = Popen::create(
        &["yes"],
        PopenConfig {
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )?;

    let result = p
        .communicate([])?
        .limit_size(100) // Only read first 100 bytes
        .read();

    match result {
        Ok((stdout, _)) => {
            println!("Read {} bytes: {:?}...", stdout.len(), &stdout[..20]);
        }
        Err(e) => println!("Error: {}", e),
    }
    p.terminate()?;
    p.wait()?;

    Ok(())
}
