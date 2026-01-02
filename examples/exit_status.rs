//! Handle various exit statuses.
//!
//! Run with: cargo run --example exit_status

use subprocess::{Exec, ExitStatus};

fn main() -> subprocess::Result<()> {
    // Successful exit
    let status = Exec::cmd("true").join()?;
    println!("true: {:?}, success={}", status, status.success());

    // Failed exit
    let status = Exec::cmd("false").join()?;
    println!("false: {:?}, success={}", status, status.success());

    // Custom exit code
    let status = Exec::shell("exit 42").join()?;
    match status {
        ExitStatus::Exited(code) => println!("exit 42: code={}", code),
        _ => println!("Unexpected status: {:?}", status),
    }

    // Check exit status from capture
    let result = Exec::shell("echo output; exit 1").capture()?;
    println!(
        "\nCaptured output: {}, exit success: {}",
        result.stdout_str().trim(),
        result.success()
    );

    // Pattern matching on exit status
    let status = Exec::cmd("ls").arg("/nonexistent").join()?;
    match status {
        ExitStatus::Exited(0) => println!("ls succeeded"),
        ExitStatus::Exited(n) => println!("ls failed with code {}", n),
        ExitStatus::Signaled(sig) => println!("ls killed by signal {}", sig),
        ExitStatus::Other(n) => println!("ls: other status {}", n),
        ExitStatus::Undetermined => println!("ls: status unknown"),
    }

    Ok(())
}
