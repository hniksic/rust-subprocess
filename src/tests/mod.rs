mod communicate;
mod exec;
mod job;
mod pipeline;
#[cfg(unix)]
mod posix;
#[cfg(windows)]
mod win32;

use crate::{Capture, Communicator, Exec, ExitStatus, Job, Pipeline, Process, Redirection};

/// Work around a NetBSD kernel race where signals sent immediately after
/// exec can be silently dropped. A brief sleep gives the child time to
/// become fully ready for signal delivery.
fn exec_signal_delay() {
    if cfg!(target_os = "netbsd") {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn public_types_are_send_and_sync() {
    assert_send_sync::<Process>();
    assert_send_sync::<Communicator>();
    assert_send_sync::<Capture>();
    assert_send_sync::<ExitStatus>();
    assert_send_sync::<Exec>();
    assert_send_sync::<Pipeline>();
    assert_send_sync::<Job>();
    assert_send_sync::<Redirection>();
}
