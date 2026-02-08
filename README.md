# subprocess

[![crates.io](https://img.shields.io/crates/v/subprocess.svg)](https://crates.io/crates/subprocess)
[![CI](https://github.com/hniksic/rust-subprocess/actions/workflows/ci.yml/badge.svg)](https://github.com/hniksic/rust-subprocess/actions/workflows/ci.yml)
[![docs.rs](https://docs.rs/subprocess/badge.svg)](https://docs.rs/subprocess)

The `subprocess` crate provides facilities for execution of and interaction with external
processes and pipelines.  It is [hosted on crates.io](https://crates.io/crates/subprocess),
with [API documentation on docs.rs](https://docs.rs/subprocess/).

The crate has minimal dependencies (only `libc` on Unix and `winapi` on Windows), and is
tested on Linux, macOS, and Windows.

## Why subprocess?

The [`std::process`](https://doc.rust-lang.org/std/process/index.html) module in the standard
library is fine for simple use cases, but it doesn't cover common scenarios such as:

* **Avoiding deadlock** when communicating with a subprocess - if you need to write to a
  subprocess's stdin while also reading its stdout and stderr, naive sequential operation can
  block forever.  `subprocess` handles this correctly using
  [poll-based](https://docs.rs/subprocess/latest/subprocess/struct.Communicator.html) I/O
  multiplexing.

* **Shell-style pipelines** - `subprocess` lets you create pipelines using the `|` operator:
  `Exec::cmd("find") | Exec::cmd("grep") | Exec::cmd("wc")`.

* **Flexible redirections** - shell-style `2>&1` is supported with
  [`Redirection::Merge`](https://docs.rs/subprocess/latest/subprocess/enum.Redirection.html#variant.Merge),
  and `>/dev/null` with
  [`Redirection::Null`](https://docs.rs/subprocess/latest/subprocess/enum.Redirection.html#variant.Null).

* **Waiting with a timeout** - `std::process::Child` offers either blocking `wait()` or
  non-blocking `try_wait()`, but nothing in-between.  `subprocess` provides timeout
  variants of its methods, such as
  [`join_timeout()`](https://docs.rs/subprocess/latest/subprocess/struct.Job.html#method.join_timeout)
  and
  [`capture_timeout()`](https://docs.rs/subprocess/latest/subprocess/struct.Job.html#method.capture_timeout).

* **Sending signals** (Unix) - `std::process::Child::kill()` only sends `SIGKILL`.
  `subprocess` lets you [send any
  signal](https://docs.rs/subprocess/latest/subprocess/unix/trait.ProcessExt.html#tymethod.send_signal)
  including `SIGTERM`, and can [signal process
  groups](https://docs.rs/subprocess/latest/subprocess/unix/trait.ProcessExt.html#tymethod.send_signal_group)
  to terminate an entire process tree.

* **Preventing zombies** - `subprocess` automatically waits on child processes when they go
  out of scope (with
  [`detach()`](https://docs.rs/subprocess/latest/subprocess/struct.Process.html#method.detach)
  to opt out), whereas `std::process::Child` does not, risking zombie process accumulation.

## Comparison with std::process

| Need | std::process | subprocess |
|------|-------------|------------|
| Wait with timeout | Loop with `try_wait()` + sleep | `wait_timeout(duration)` |
| Write stdin while reading stdout | Manual threading or async | `capture()` handles it |
| Pipelines | Manual pipe setup | `cmd1 \| cmd2 \| cmd3` |
| Merge stderr into stdout | Not supported | `Redirection::Merge` |
| Send SIGTERM (Unix) | Only `kill()` (SIGKILL) | `send_signal(SIGTERM)` |
| Auto-cleanup on drop | No (zombies possible) | Yes (waits by default) |

## API Overview

The API has two layers:

* **[`Exec`](https://docs.rs/subprocess/latest/subprocess/struct.Exec.html) /
  [`Pipeline`](https://docs.rs/subprocess/latest/subprocess/struct.Pipeline.html)** -
  builder-pattern API for configuring processes and pipelines.  Convenience methods like
  `join()` and `capture()` configure, start, and collect results in one call.

* **[`Job`](https://docs.rs/subprocess/latest/subprocess/struct.Job.html)** - handle
  for a running process or pipeline, returned by `start()`.  Provides timeout-aware methods
  like `join_timeout()` and `capture_timeout()`, as well as `communicate()`.
  [`Capture`](https://docs.rs/subprocess/latest/subprocess/struct.Capture.html) is
  returned by `capture()` and holds the collected stdout, stderr, and exit status. 

## Examples

### Basic execution

Execute a command and wait for it to complete:

```rust
Exec::cmd("umount").arg(dirname).checked().join()?;
```

`join()` starts the command and waits for it to finish, returning the exit
status. `checked()` ensures error is returned for non-zero exit status.

To prevent quoting issues and shell injection attacks, `subprocess` doesn't spawn a shell
unless explicitly requested.  To execute a command through the OS shell, use
`Exec::shell`:

```rust
Exec::shell("shutdown -h now").join()?;
```

### Capturing output

Capture the stdout and stderr of a command, and print the stdout:

```rust
let rustver = Exec::shell("rustc --version").capture()?.stdout_str();
```

Capture stdout and stderr merged together:

```rust
let out_and_err = Exec::cmd("cargo").arg("check")
  .stderr(Redirection::Merge)  // 2>&1
  .capture()?
  .stdout_str();
```

### Feeding input

`capture()` can simultaneously feed data to stdin and read stdout/stderr, avoiding the
deadlock that would result from doing these sequentially:

```rust
let lines = Exec::cmd("sqlite3")
    .arg(db_path)
    .stdin("SELECT name FROM users WHERE active = 1;")
    .capture()?
    .stdout_str();
```

### Streaming

Get stdout as a `Read` trait object (like C's `popen`):

```rust
let stream = Exec::cmd("find").arg("/").stream_stdout()?;
// Use stream.read_to_string(), BufReader::new(stream).lines(), etc.
```

### Pipelines

Create pipelines using the `|` operator:

```rust
let dir_checksum = (Exec::shell("find . -type f") | Exec::cmd("sort") | Exec::cmd("sha1sum"))
    .capture()?
    .stdout_str();
```

Pipeline supports the same methods for interacting with the subprocess as with a single
started command.

### Timeouts

Capture with timeout:

```rust
let response = Exec::cmd("curl").arg("-s").arg(url)
    .start()?
    .capture_timeout(Duration::from_secs(10))?
    .stdout_str();
```

`communicate()` can be used for more sophisticated control over timeouts, such as reading
with a time or size limit:

```rust
let mut comm = Exec::cmd("ping").arg("example.com").detached().communicate()?;
let (out, _) = comm
    .limit_time(Duration::from_secs(5))
    .read_string()?;
```

### Termination

Give the process some time to run, then terminate if needed:

```rust
let mut started = Exec::cmd("sleep").arg("10").start()?;
match started.wait_timeout(Duration::from_secs(1))? {
    Some(status) => println!("finished: {:?}", status),
    None => {
        started.terminate()?;
        started.wait()?;
    }
}
```

## License

`subprocess` is distributed under the terms of both the MIT license and the Apache License
(Version 2.0).  See [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT) for
details.  Contributing changes is assumed to signal agreement with these licensing terms.
