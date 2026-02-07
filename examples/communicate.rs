//! Bidirectional communication with a subprocess.
//!
//! Run with: cargo run --example communicate

use std::time::Duration;
use subprocess::{Exec, Redirection};

fn main() -> std::io::Result<()> {
    // Basic communicate: send input, receive output
    let mut handle = Exec::cmd("cat")
        .stdin("Hello, cat!")
        .stdout(Redirection::Pipe)
        .start()?;

    let (stdout, _stderr) = handle.communicate().read()?;
    println!("cat said: {}", String::from_utf8_lossy(&stdout));
    handle.wait()?;

    // Communicate with timeout using Communicator
    println!("\nCommunicating with timeout:");
    let mut handle = Exec::cmd("cat")
        .stdin("data with timeout")
        .stdout(Redirection::Pipe)
        .start()?;

    let result = handle
        .communicate()
        .limit_time(Duration::from_secs(5))
        .read();

    match result {
        Ok((stdout, _)) => println!("Got: {}", String::from_utf8_lossy(&stdout)),
        Err(e) => println!("Error: {}", e),
    }
    handle.wait()?;

    // Communicate with size limit
    println!("\nCommunicating with size limit:");
    let mut handle = Exec::cmd("yes").stdout(Redirection::Pipe).start()?;

    let result = handle
        .communicate()
        .limit_size(100) // Only read first 100 bytes
        .read();

    match result {
        Ok((stdout, _)) => {
            println!("Read {} bytes: {:?}...", stdout.len(), &stdout[..20]);
        }
        Err(e) => println!("Error: {}", e),
    }
    handle.terminate()?;
    handle.wait()?;

    Ok(())
}
