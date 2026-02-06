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
    assert_send_sync::<Communicator>();
    assert_send_sync::<Capture>();
    assert_send_sync::<ExitStatus>();
    assert_send_sync::<Exec>();
    assert_send_sync::<Pipeline>();
    assert_send_sync::<PopenConfig>();
    assert_send_sync::<Redirection>();
}
