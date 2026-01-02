//! Bidirectional communication with a subprocess.
//!
//! Run with: cargo run --example communicate

use std::time::Duration;
use subprocess::{Popen, PopenConfig, Redirection};

fn main() -> subprocess::Result<()> {
    // Basic communicate: send input, receive output
    let mut p = Popen::create(
        &["cat"],
        PopenConfig {
            stdin: Redirection::Pipe,
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )?;

    let (stdout, _stderr) = p.communicate(Some("Hello, cat!"))?;
    println!("cat said: {}", stdout.unwrap());
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
        .communicate_start(Some(b"data with timeout".to_vec()))
        .limit_time(Duration::from_secs(5))
        .read();

    match result {
        Ok((stdout, _)) => println!("Got: {}", String::from_utf8_lossy(&stdout.unwrap())),
        Err(e) => println!("Error: {}", e.error),
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
        .communicate_start(None)
        .limit_size(100) // Only read first 100 bytes
        .read();

    match result {
        Ok((stdout, _)) => {
            let data = stdout.unwrap();
            println!("Read {} bytes: {:?}...", data.len(), &data[..20]);
        }
        Err(e) => println!("Error: {}", e.error),
    }
    p.terminate()?;
    p.wait()?;

    Ok(())
}
