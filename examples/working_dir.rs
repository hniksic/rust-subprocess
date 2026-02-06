//! Set the working directory for a subprocess.
//!
//! Run with: cargo run --example working_dir

use subprocess::Exec;

fn main() -> std::io::Result<()> {
    // Run command in a specific directory
    let output = Exec::cmd("pwd").cwd("/tmp").capture()?.stdout_str();

    println!("Working directory: {}", output.trim());

    // List files in a specific directory
    let output = Exec::cmd("ls").cwd("/").capture()?.stdout_str();

    println!("\nFiles in root directory:");
    for file in output.lines().take(5) {
        println!("  {}", file);
    }
    println!("  ...");

    // Relative paths are resolved relative to cwd
    let output = Exec::shell("ls ..").cwd("/usr/bin").capture()?.stdout_str();

    println!("\nParent of /usr/bin contains:");
    for file in output.lines().take(3) {
        println!("  {}", file);
    }

    Ok(())
}
