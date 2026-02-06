//! Run detached (background) processes.
//!
//! Run with: cargo run --example detached

use std::time::Duration;
use subprocess::Exec;

fn main() -> std::io::Result<()> {
    // Start a detached process - won't be waited on drop
    println!("Starting detached process...");
    let popen = Exec::cmd("sleep").arg("0.1").detached().popen()?;

    println!("Process started with PID: {:?}", popen.pid());
    println!("Dropping Popen without waiting...");
    drop(popen);
    println!("Popen dropped, process may still be running");

    // Start and explicitly wait
    println!("\nStarting another process...");
    let mut popen = Exec::cmd("sleep").arg("0.1").detached().popen()?;

    println!("Waiting explicitly...");
    let status = popen.wait()?;
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
