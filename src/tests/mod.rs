mod communicate;
mod exec;
mod pipeline;
#[cfg(unix)]
mod posix;
mod started;
#[cfg(windows)]
mod win32;

use crate::{Capture, Communicator, Exec, ExitStatus, Pipeline, Process, Redirection, Started};

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn public_types_are_send_and_sync() {
    assert_send_sync::<Process>();
    assert_send_sync::<Communicator<Vec<u8>>>();
    assert_send_sync::<Communicator<&[u8]>>();
    assert_send_sync::<Capture>();
    assert_send_sync::<ExitStatus>();
    assert_send_sync::<Exec>();
    assert_send_sync::<Pipeline>();
    assert_send_sync::<Started>();
    assert_send_sync::<Redirection>();
}
