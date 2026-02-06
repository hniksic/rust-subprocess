//! Control subprocess environment variables.
//!
//! Run with: cargo run --example environment

use subprocess::Exec;

fn main() -> std::io::Result<()> {
    // Set a single environment variable
    let output = Exec::shell("echo $GREETING")
        .env("GREETING", "Hello from subprocess!")
        .capture()?
        .stdout_str();

    println!("With custom env: {}", output.trim());

    // Set multiple environment variables
    let output = Exec::shell("echo $FIRST $SECOND")
        .env_extend([("FIRST", "Hello"), ("SECOND", "World")])
        .capture()?
        .stdout_str();

    println!("Multiple vars: {}", output.trim());

    // Clear environment and set only specific variables
    let output = Exec::shell("env | wc -l")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("ONLY_VAR", "value")
        .capture()?
        .stdout_str();

    println!("Minimal env has {} variables", output.trim());

    // Remove a specific variable
    let output = Exec::shell("echo ${HOME:-not set}")
        .env_remove("HOME")
        .capture()?
        .stdout_str();

    println!("Without HOME: {}", output.trim());

    Ok(())
}
