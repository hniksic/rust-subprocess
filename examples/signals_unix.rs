//! Unix-specific: Send signals to processes.
//!
//! Run with: cargo run --example signals_unix

#[cfg(unix)]
fn main() -> std::io::Result<()> {
    use subprocess::unix::StartedExt;
    use subprocess::{Exec, ExecExt};

    // Start a long-running process
    let handle = Exec::cmd("sleep").arg("100").start()?;
    println!("Started sleep with PID {}", handle.pid());

    // Send SIGTERM (graceful termination)
    handle.send_signal(libc::SIGTERM)?;
    println!("Sent SIGTERM");

    // Start another process in its own process group
    let handle = Exec::cmd("sleep").arg("100").setpgid().start()?;
    println!("\nStarted sleep in new process group, PID {}", handle.pid());

    // Send signal to the entire process group
    handle.send_signal_group(libc::SIGKILL)?;
    println!("Sent SIGKILL to process group");

    // Demonstrate terminate vs kill
    let handle = Exec::cmd("sleep").arg("100").start()?;
    println!("\nStarted another sleep, PID {}", handle.pid());

    // terminate() sends SIGTERM
    handle.terminate()?;
    let status = handle.wait()?;
    println!("After terminate: {:?}", status);

    let handle = Exec::cmd("sleep").arg("100").start()?;
    println!("\nStarted another sleep, PID {}", handle.pid());

    // kill() sends SIGKILL
    handle.kill()?;
    let status = handle.wait()?;
    println!("After kill: {:?}", status);

    Ok(())
}

#[cfg(not(unix))]
fn main() {
    println!("This example only runs on Unix systems.");
}
