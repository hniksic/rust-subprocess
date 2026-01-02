//! Unix-specific: Send signals to processes.
//!
//! Run with: cargo run --example signals_unix

#[cfg(unix)]
fn main() -> subprocess::Result<()> {
    use subprocess::unix::PopenExt;
    use subprocess::{Popen, PopenConfig};

    // Start a long-running process
    let p = Popen::create(&["sleep", "100"], PopenConfig::default())?;
    println!("Started sleep with PID {:?}", p.pid());

    // Send SIGTERM (graceful termination)
    p.send_signal(libc::SIGTERM)?;
    println!("Sent SIGTERM");

    // Start another process in its own process group
    let p = Popen::create(
        &["sleep", "100"],
        PopenConfig {
            setpgid: true,
            ..Default::default()
        },
    )?;
    println!("\nStarted sleep in new process group, PID {:?}", p.pid());

    // Send signal to the entire process group
    p.send_signal_group(libc::SIGKILL)?;
    println!("Sent SIGKILL to process group");

    // Demonstrate terminate vs kill
    let mut p = Popen::create(&["sleep", "100"], PopenConfig::default())?;
    println!("\nStarted another sleep, PID {:?}", p.pid());

    // terminate() sends SIGTERM
    p.terminate()?;
    let status = p.wait()?;
    println!("After terminate: {:?}", status);

    let mut p = Popen::create(&["sleep", "100"], PopenConfig::default())?;
    println!("\nStarted another sleep, PID {:?}", p.pid());

    // kill() sends SIGKILL
    p.kill()?;
    let status = p.wait()?;
    println!("After kill: {:?}", status);

    Ok(())
}

#[cfg(not(unix))]
fn main() {
    println!("This example only runs on Unix systems.");
}
