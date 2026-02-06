mod builder;
mod communicate;
mod popen;
#[cfg(unix)]
mod posix;
#[cfg(windows)]
mod win32;

use crate::{Capture, Communicator, Exec, ExitStatus, Pipeline, Popen, PopenConfig, Redirection};

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn public_types_are_send_and_sync() {
    assert_send_sync::<Popen>();
    assert_send_sync::<Communicator<Vec<u8>>>();
    assert_send_sync::<Communicator<&[u8]>>();
    assert_send_sync::<Capture>();
    assert_send_sync::<ExitStatus>();
    assert_send_sync::<Exec>();
    assert_send_sync::<Pipeline>();
    assert_send_sync::<PopenConfig>();
    assert_send_sync::<Redirection>();
}

#[test]
fn exit_status_code() {
    assert_eq!(ExitStatus::Exited(0).code(), Some(0));
    assert_eq!(ExitStatus::Exited(1).code(), Some(1));
    assert_eq!(ExitStatus::Signaled(9).code(), None);
    assert_eq!(ExitStatus::Other(-1).code(), None);
    assert_eq!(ExitStatus::Undetermined.code(), None);
}

#[test]
fn exit_status_display() {
    assert_eq!(ExitStatus::Exited(0).to_string(), "exit code 0");
    assert_eq!(ExitStatus::Exited(1).to_string(), "exit code 1");
    assert_eq!(ExitStatus::Signaled(9).to_string(), "signal 9");
    assert_eq!(ExitStatus::Other(-1).to_string(), "exit status -1");
    assert_eq!(
        ExitStatus::Undetermined.to_string(),
        "undetermined exit status"
    );
}
