//! Handle subprocess timeouts.
//!
//! Run with: cargo run --example timeout

use std::time::Duration;
use subprocess::{Exec, Redirection};

fn main() -> std::io::Result<()> {
    // Using wait_timeout on a process
    println!("Waiting with timeout...");

    let handle = Exec::cmd("sleep").arg("10").start()?;

    match handle.wait_timeout(Duration::from_millis(100))? {
        Some(status) => println!("Process exited: {:?}", status),
        None => {
            println!("Timeout! Process still running, terminating...");
            handle.terminate()?;
            handle.wait()?;
            println!("Process terminated.");
        }
    }

    // Polling without blocking
    println!("\nPolling a quick command...");
    let handle = Exec::cmd("echo")
        .arg("quick")
        .stdout(Redirection::Pipe)
        .start()?;

    // Poll until done
    loop {
        if let Some(status) = handle.poll() {
            println!("Command finished with: {:?}", status);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    Ok(())
}
