use std::ffi::OsStr;

use crate::{ExitStatus, Popen, PopenConfig, Redirection};

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
    let (out, _err) = p.communicate(None).unwrap();
    assert_eq!(out.unwrap().trim(), "foobar");
}

#[test]
fn err_terminate() {
    let mut p = Popen::create(&["sleep", "5"], PopenConfig::default()).unwrap();
    assert!(p.poll().is_none());
    p.terminate().unwrap();
    assert!(p.wait().unwrap() == ExitStatus::Exited(1));
}
