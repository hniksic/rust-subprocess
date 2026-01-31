mod builder;
mod communicate;
mod popen;
#[cfg(unix)]
mod posix;
#[cfg(windows)]
mod win32;

use crate::{CaptureData, CommunicateError, Communicator, ExitStatus, NullFile, Popen, PopenError};

fn assert_send_sync<T: Send + Sync>() {}

// Exec, Pipeline, PopenConfig, and Redirection are intentionally
// !Send because Redirection contains Rc<File>.

#[test]
fn public_types_are_send_and_sync() {
    assert_send_sync::<Popen>();
    assert_send_sync::<Communicator>();
    assert_send_sync::<CaptureData>();
    assert_send_sync::<ExitStatus>();
    assert_send_sync::<PopenError>();
    assert_send_sync::<CommunicateError>();
    assert_send_sync::<NullFile>();
}
