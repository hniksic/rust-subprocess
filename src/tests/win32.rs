use std::ffi::OsStr;

use crate::ExitStatus;
use crate::{Popen, PopenConfig, Redirection};

#[test]
fn setup_executable() {
    // Test that PopenConfig::executable overrides the actual executable while argv[0] is
    // passed to the process. We run PowerShell with executable override, and have it
    // print its argv[0] which should be "foobar", not "powershell".
    let mut p = Popen::create(
        &[
            "foobar",
            "-Command",
            "[Environment]::GetCommandLineArgs()[0]",
        ],
        PopenConfig {
            executable: Some(OsStr::new("powershell.exe").to_owned()),
            stdout: Redirection::Pipe,
            ..Default::default()
        },
    )
    .unwrap();
    let (out, _err) = p.communicate([]).unwrap().read_string().unwrap();
    assert_eq!(out.trim(), "foobar");
}

#[test]
fn err_terminate() {
    let mut p = Popen::create(&["sleep", "5"], PopenConfig::default()).unwrap();
    assert!(p.poll().is_none());
    p.terminate().unwrap();
    assert_eq!(p.wait().unwrap().code(), Some(1));
}

#[test]
fn exit_status_code() {
    assert_eq!(ExitStatus::from_raw(0).code(), Some(0));
    assert_eq!(ExitStatus::from_raw(1).code(), Some(1));
    assert_eq!(ExitStatus::from_raw(42).code(), Some(42));
}

#[test]
fn exit_status_display() {
    assert_eq!(ExitStatus::from_raw(0).to_string(), "exit code 0");
    assert_eq!(ExitStatus::from_raw(1).to_string(), "exit code 1");
    assert_eq!(ExitStatus::from_raw(42).to_string(), "exit code 42");
}
