//! Feed data to a subprocess via stdin.
//!
//! Run with: cargo run --example stdin_feed

use subprocess::Exec;

fn main() -> std::io::Result<()> {
    // Feed string data to sort command
    let input = "banana\napple\ncherry\ndate\n";
    let sorted = Exec::cmd("sort").stdin(input).capture()?.stdout_str();

    println!("Sorted input:\n{sorted}");

    // Feed binary data
    let numbers: Vec<u8> = vec![3, 1, 4, 1, 5, 9, 2, 6];
    let hex_output = Exec::cmd("xxd").stdin(numbers).capture()?.stdout_str();

    println!("Hex dump:\n{hex_output}");

    // Pipeline with stdin data
    let result = (Exec::cmd("cat") | Exec::cmd("rev"))
        .stdin("hello\nworld\n")
        .capture()?
        .stdout_str();

    println!("Reversed lines:\n{result}");

    Ok(())
}
