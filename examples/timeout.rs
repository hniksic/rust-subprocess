//! Handle subprocess timeouts.
//!
//! Run with: cargo run --example timeout

use std::time::Duration;
use subprocess::{Popen, PopenConfig, Redirection};

fn main() -> std::io::Result<()> {
    // Using wait_timeout on Popen
    println!("Waiting with timeout...");

    let mut p = Popen::create(
        &["sleep", "10"],
        PopenConfig {
            ..Default::default()
        },
    )?;

    match p.wait_timeout(Duration::from_millis(100))? {
        Some(status) => println!("Process exited: {:?}", status),
        None => {
            println!("Timeout! Process still running, terminating...");
            p.terminate()?;
            p.wait()?;
            println!("Process terminated.");
        }
    }

    // Polling without blocking
    println!("\nPolling a quick command...");
    let mut p = Popen::create(
        &["echo", "quick"],
        PopenConfig {
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )?;

    // Poll until done
    loop {
        if let Some(status) = p.poll() {
            println!("Command finished with: {:?}", status);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    Ok(())
}
