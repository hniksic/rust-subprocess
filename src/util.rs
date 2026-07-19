use std::time::Duration;

/// Converts a timeout to whole milliseconds, rounding up so that a timed wait never
/// waits less than requested. In particular, a sub-millisecond timeout must not
/// degenerate into a non-blocking check (timeout of 0).
pub(crate) fn duration_to_ms_ceil(timeout: Duration) -> u128 {
    timeout.as_nanos().div_ceil(1_000_000)
}
