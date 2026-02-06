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

* **Merging stdout and stderr** - shell-style `2>&1` redirection is directly supported with
  [`Redirection::Merge`](https://docs.rs/subprocess/latest/subprocess/enum.Redirection.html#variant.Merge),
  which has no equivalent in `std::process::Stdio`.

* **Waiting with a timeout** - `std::process::Child` offers either blocking `wait()` or
  non-blocking `try_wait()`, but nothing in-between.  `subprocess` provides
  [`wait_timeout()`](https://docs.rs/subprocess/latest/subprocess/struct.Popen.html#method.wait_timeout).

* **Sending signals** (Unix) - `std::process::Child::kill()` only sends `SIGKILL`.
  `subprocess` lets you [send any
  signal](https://docs.rs/subprocess/latest/subprocess/unix/trait.PopenExt.html#tymethod.send_signal)
  including `SIGTERM`, and can [signal process
  groups](https://docs.rs/subprocess/latest/subprocess/unix/trait.PopenExt.html#tymethod.send_signal_group)
  to terminate an entire process tree.

* **Preventing zombies** - `subprocess` automatically waits on child processes when they go
  out of scope (with
  [`detach()`](https://docs.rs/subprocess/latest/subprocess/struct.Popen.html#method.detach)
  to opt out), whereas `std::process::Child` does not, risking zombie process accumulation.

## Comparison with std::process

| Need | std::process | subprocess |
|------|-------------|------------|
| Wait with timeout | Loop with `try_wait()` + sleep | `wait_timeout(duration)` |
| Write stdin while reading stdout | Manual threading or async | `communicate()` handles it |
| Pipelines | Manual pipe setup | `cmd1 \| cmd2 \| cmd3` |
| Merge stderr into stdout | Not supported | `Redirection::Merge` |
| Send SIGTERM (Unix) | Only `kill()` (SIGKILL) | `send_signal(SIGTERM)` |
| Signal process group (Unix) | Not supported | `send_signal_group()` |
| Auto-cleanup on drop | No (zombies possible) | Yes (waits by default) |

## API Overview

The API has two levels:

* **High-level:** The
  [`Exec`](https://docs.rs/subprocess/latest/subprocess/struct.Exec.html) builder provides a
  convenient interface for spawning processes and pipelines, with methods like `join()`,
  `capture()`, `stream_stdout()`, etc.

* **Low-level:** The
  [`Popen`](https://docs.rs/subprocess/latest/subprocess/struct.Popen.html) struct offers
  direct control over the process lifecycle.  `Exec` creates `Popen` instances which can then
  be manipulated directly.

## Examples

### Basic execution

Execute a command and wait for it to complete:

```rust
let exit_status = Exec::cmd("umount").arg(dirname).join()?;
assert!(exit_status.success());
```

To prevent quoting issues and shell injection attacks, `subprocess` does not spawn a shell
unless explicitly requested.  To execute a command through the OS shell, use `Exec::shell`:

```rust
Exec::shell("shutdown -h now").join()?;
```

### Capturing output

Capture the output of a command:

```rust
let out = Exec::cmd("ls")
  .stdout(Redirection::Pipe)
  .capture()?
  .stdout_str();
```

Capture both stdout and stderr merged together:

```rust
let out_and_err = Exec::cmd("ls")
  .stdout(Redirection::Pipe)
  .stderr(Redirection::Merge)  // 2>&1
  .capture()?
  .stdout_str();
```

### Feeding input

Provide input data and capture output:

```rust
let out = Exec::cmd("sort")
  .stdin("b\nc\na\n")
  .stdout(Redirection::Pipe)
  .capture()?
  .stdout_str();
assert_eq!(out, "a\nb\nc\n");
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
let exit_status =
  (Exec::shell("ls *.bak") | Exec::cmd("xargs").arg("rm")).join()?;
```

Capture the output of a pipeline:

```rust
let dir_checksum = {
    Exec::shell("find . -type f") | Exec::cmd("sort") | Exec::cmd("sha1sum")
}.capture()?.stdout_str();
```

### Waiting with timeout

Give the process some time to run, then terminate if needed:

```rust
let mut p = Exec::cmd("sleep").arg("10").popen()?;
if let Some(status) = p.wait_timeout(Duration::from_secs(1))? {
    println!("finished: {:?}", status);
} else {
    println!("timed out, terminating");
    p.terminate()?;
    p.wait()?;
}
```

### Communicating with deadlock prevention

When you need to write to stdin and read from stdout/stderr simultaneously:

```rust
let mut p = Popen::create(&["cat"], PopenConfig {
    stdin: Redirection::Pipe,
    stdout: Redirection::Pipe,
    ..Default::default()
})?;

// communicate() handles the write/read interleaving to avoid deadlock
let (out, _err) = p.communicate("hello world").read_string()?;
assert_eq!(out, "hello world");
```

With a timeout:

```rust
let mut comm = Exec::cmd("slow-program")
    .stdin("input")
    .stdout(Redirection::Pipe)
    .communicate()?
    .limit_time(Duration::from_secs(5));

match comm.read_string() {
    Ok((stdout, stderr)) => println!("got: {:?}", stdout),
    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
        println!("timed out, partial: {:?}", e.capture);
    }
    Err(e) => return Err(e.into()),
}
```

### Sending signals (Unix)

Send a signal other than SIGKILL:

```rust
use subprocess::unix::PopenExt;

let mut p = Exec::cmd("sleep").arg("100").popen()?;
p.send_signal(libc::SIGTERM)?;  // graceful termination
p.wait()?;
```

Terminate an entire process tree using process groups:

```rust
use subprocess::unix::PopenExt;

// Start child in its own process group
let mut p = Popen::create(&["sh", "-c", "sleep 100 & sleep 100"], PopenConfig {
    setpgid: true,
    ..Default::default()
})?;

// Signal the entire process group
p.send_signal_group(libc::SIGTERM)?;
p.wait()?;
```

### Low-level Popen interface

For full control over the process lifecycle:

```rust
let mut p = Popen::create(&["command", "arg1", "arg2"], PopenConfig {
    stdout: Redirection::Pipe,
    ..Default::default()
})?;

// Read stdout directly
let (out, err) = p.communicate([]).read_string()?;

// Check if still running
if let Some(exit_status) = p.poll() {
    println!("finished: {:?}", exit_status);
} else {
    println!("still running, terminating");
    p.terminate()?;
}
```

## License

`subprocess` is distributed under the terms of both the MIT license and the Apache License
(Version 2.0).  See [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT) for
details.  Contributing changes is assumed to signal agreement with these licensing terms.
