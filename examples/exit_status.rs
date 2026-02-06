//! Handle various exit statuses.
//!
//! Run with: cargo run --example exit_status

use subprocess::Exec;

fn main() -> std::io::Result<()> {
    // Successful exit
    let status = Exec::cmd("true").join()?;
    println!("true: {:?}, success={}", status, status.success());

    // Failed exit
    let status = Exec::cmd("false").join()?;
    println!("false: {:?}, success={}", status, status.success());

    // Custom exit code
    let status = Exec::shell("exit 42").join()?;
    if let Some(code) = status.code() {
        println!("exit 42: code={}", code);
    } else if let Some(signal) = status.signal() {
        println!("exit 42: killed by signal {}", signal);
    } else {
        println!("exit 42: undetermined status");
    }

    // Check exit status from capture
    let result = Exec::shell("echo output; exit 1").capture()?;
    println!(
        "\nCaptured output: {}, exit success: {}",
        result.stdout_str().trim(),
        result.success()
    );

    // Method-based status checks
    let status = Exec::cmd("ls").arg("/nonexistent").join()?;
    if status.success() {
        println!("ls succeeded");
    } else if let Some(code) = status.code() {
        println!("ls failed with code {}", code);
    } else if let Some(signal) = status.signal() {
        println!("ls killed by signal {}", signal);
    } else {
        println!("ls: status unknown");
    }

    Ok(())
}
