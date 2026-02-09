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

Compared to [`std::process`](https://doc.rust-lang.org/std/process/index.html), the crate
provides additional features:

* **The capture and communicate** [family of methods](Job::capture) for deadlock-free
  capturing of subprocess output/error, while simultaneously feeding data to its standard
  input.  Capturing supports optional timeout and read size limit.

* **OS-level pipelines** using the `|` operator: `Exec::cmd("find") | Exec::cmd("grep") |
  Exec::cmd("wc")`. There is no difference between interacting with pipelines and with a
  single process.

* **Flexible redirection** options, such as connecting standard streams to arbitrary sources,
  including those implemented in Rust, or merging output streams like shell's `2>&1` and
  `1>&2` operators.

* **Non-blocking and timeout methods** to wait on the process: `subprocess` provides
  timeout variants of its methods, such as
  [`wait_timeout()`](https://docs.rs/subprocess/latest/subprocess/struct.Job.html#method.wait_timeout),
  [`join_timeout()`](https://docs.rs/subprocess/latest/subprocess/struct.Job.html#method.join_timeout)
  and
  [`capture_timeout()`](https://docs.rs/subprocess/latest/subprocess/struct.Job.html#method.capture_timeout).

| Need | std::process | subprocess |
|------|-------------|------------|
| Pipelines | Manual pipe setup | `cmd1 \| cmd2 \| cmd3` |
| Write stdin while capturing stdout | Manual threading or async | `capture()` handles it |
| Wait with timeout | Loop with `try_wait()` + sleep | `wait_timeout(duration)` |
| Merge stderr into stdout | Not supported | `Redirection::Merge` |
| Share process handle across threads | `Arc<Mutex<Child>>` | Clone the `Process` handle |
| Send SIGTERM (Unix) | Only `kill()` (SIGKILL) | `send_signal(SIGTERM)` |
| Auto-cleanup on drop | No (zombies possible) | Yes (waits by default) |

## API Overview

The API has two layers:

* **[`Exec`](https://docs.rs/subprocess/latest/subprocess/struct.Exec.html) /
  [`Pipeline`](https://docs.rs/subprocess/latest/subprocess/struct.Pipeline.html)** -
  builder-pattern API for configuring processes and pipelines.  Convenience methods like
  [`join()`](https://docs.rs/subprocess/latest/subprocess/struct.Exec.html#method.join)
  and
  [`start()`](https://docs.rs/subprocess/latest/subprocess/struct.Exec.html#method.capture)`
  configure, start, and collect results in one call.

* **[`Job`](https://docs.rs/subprocess/latest/subprocess/struct.Job.html)** - interface to
  a started process or pipeline, returned by
  [`start()`](https://docs.rs/subprocess/latest/subprocess/struct.Exec.html#method.start).
  Contains input and output streams, and provides methods for inspecting the process
  status and capturing output, and timeout-aware waiting methods.

## Examples

### Execution

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

`capture()` can simultaneously feed data to stdin and read stdout/stderr, avoiding the
deadlock that would result from doing these sequentially:

```rust
let lines = Exec::cmd("sqlite3")
    .arg(db_path)
    .stdin("SELECT name FROM users WHERE active = 1;")
    .capture()?
    .stdout_str();
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

### Streaming

Get stdout as a `Read` trait object (like C's `popen`):

```rust
let stream = Exec::cmd("find").arg("/").stream_stdout()?;
// Use stream.read_to_string(), BufReader::new(stream).lines(), etc.
```

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
