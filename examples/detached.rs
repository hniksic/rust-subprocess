//! Run detached (background) processes.
//!
//! Run with: cargo run --example detached

use std::time::Duration;
use subprocess::Exec;

fn main() -> std::io::Result<()> {
    // Start a detached process - won't be waited on drop
    println!("Starting detached process...");
    let handle = Exec::cmd("sleep").arg("0.1").detached().start()?;

    println!("Process started with PID: {}", handle.pid());
    println!("Dropping handle without waiting...");
    drop(handle);
    println!("Handle dropped, process may still be running");

    // Start and explicitly wait
    println!("\nStarting another process...");
    let mut handle = Exec::cmd("sleep").arg("0.1").detached().start()?;

    println!("Waiting explicitly...");
    let status = handle.wait()?;
    println!("Process finished: {:?}", status);

    // Detached with streaming - useful for long-running processes
    println!("\nStreaming from detached process:");
    let stream = Exec::shell("for i in 1 2 3; do echo $i; sleep 0.05; done")
        .detached()
        .stream_stdout()?;

    use std::io::{BufRead, BufReader};
    for line in BufReader::new(stream).lines() {
        println!("  Got: {}", line.unwrap());
    }

    std::thread::sleep(Duration::from_millis(100));
    println!("Done!");

    Ok(())
}
