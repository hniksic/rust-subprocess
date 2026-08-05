extern crate subprocess;

use std::io::Read;
use subprocess::{Exec, Redirection};

fn argv_echo_path() -> &'static str {
    // Provided by cargo when building integration tests; doesn't depend on the target
    // dir layout.
    env!("CARGO_BIN_EXE_argv-echo")
}

#[test]
fn escape_args() {
    // This is mostly relevant for Windows: test whether
    // assemble_cmdline does a good job with arguments that require
    // escaping.
    for &arg in &[
        "x", "", " ", "  ", r" \ ", r" \\ ", r" \\\ ", r#"""#, r#""""#, r#"\"\\""#, "æ÷", "šđ",
        "本", "❤", "☃",
    ] {
        let mut handle = Exec::cmd(argv_echo_path())
            .arg(arg)
            .stdout(Redirection::Pipe)
            .start()
            .unwrap();
        let mut output = handle.stdout.take().unwrap();
        let mut output_str = String::new();
        output.read_to_string(&mut output_str).unwrap();
        assert_eq!(output_str, arg);
    }
}
