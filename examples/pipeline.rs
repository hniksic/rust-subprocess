//! Demonstrate OS-level pipelines.
//!
//! Run with: cargo run --example pipeline

use subprocess::Exec;

fn main() -> subprocess::Result<()> {
    // Simple pipeline: generate data, transform it, capture output
    let data = (Exec::cmd("echo").args(&["cherry", "apple", "banana"])
        | Exec::cmd("tr").args(&[" ", "\n"])
        | Exec::cmd("sort"))
    .capture()?
    .stdout_str();

    println!("Sorted fruits:\n{data}");

    // Pipeline with shell commands
    let result = (Exec::shell("echo 'hello world'")
        | Exec::shell("tr '[:lower:]' '[:upper:]'")
        | Exec::shell("rev"))
    .capture()?
    .stdout_str();

    println!("Transformed: {}", result.trim());

    // Build pipeline dynamically
    let commands = vec![
        Exec::shell("echo one two three"),
        Exec::shell("tr ' ' '\\n'"),
        Exec::cmd("wc").arg("-l"),
    ];

    let line_count = subprocess::Pipeline::from_exec_iter(commands)
        .capture()?
        .stdout_str();

    println!("Line count: {}", line_count.trim());

    Ok(())
}
