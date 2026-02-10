# subprocess

[![crates.io](https://img.shields.io/crates/v/subprocess.svg)](https://crates.io/crates/subprocess)
[![CI](https://github.com/hniksic/rust-subprocess/actions/workflows/ci.yml/badge.svg)](https://github.com/hniksic/rust-subprocess/actions/workflows/ci.yml)
[![docs.rs](https://docs.rs/subprocess/badge.svg)](https://docs.rs/subprocess)

The `subprocess` crate provides facilities for execution of and interaction with external
processes and pipelines.  It is [hosted on crates.io](https://crates.io/crates/subprocess),
with [API documentation on docs.rs](https://docs.rs/subprocess/).

The crate has minimal dependencies (only `libc` on Unix and `winapi` on Windows), and is
tested on Linux, macOS, and Windows.

If you're upgrading from version 0.2, see the [migration guide](MIGRATION.md).

## Why subprocess?

Compared to [`std::process`](https://doc.rust-lang.org/std/process/index.html), the crate
provides additional features:

* **OS-level pipelines** using the `|` operator: `Exec::cmd("find") | 
  Exec::cmd("grep").arg(r"\.py$") | Exec::cmd("wc")`. There is no difference between
  interacting with pipelines and with a single process.

* **Capture and communicate** [family of
  methods](https://docs.rs/subprocess/latest/subprocess/struct.Job.html#method.capture)
  for deadlock-free capturing of subprocess output/error, while simultaneously feeding
  data to its standard input.  Capturing supports optional timeout and read size limit.

* **Flexible redirection** options, such as connecting standard input to arbitrary data
  sources, and merging output streams like shell's `2>&1` and `1>&2` operators.

* **Non-blocking and timeout methods** to wait on the process: `subprocess` provides
  timeout variants of its methods, such as
  [`wait_timeout()`](https://docs.rs/subprocess/latest/subprocess/struct.Job.html#method.wait_timeout),
  [`join_timeout()`](https://docs.rs/subprocess/latest/subprocess/struct.Job.html#method.join_timeout)
  and
  [`capture_timeout()`](https://docs.rs/subprocess/latest/subprocess/struct.Job.html#method.capture_timeout).

* Various conveniences, such as `checked()` to flag non-zero exit status as error,
  thread-safe and cloneable process handle with `&self` methods, support for `setpgid()`
  on individual commands and on the pipeline, and many others.

## API Overview

The API consists of two main components:

* **[`Exec`](https://docs.rs/subprocess/latest/subprocess/struct.Exec.html) /
  [`Pipeline`](https://docs.rs/subprocess/latest/subprocess/struct.Pipeline.html)** -
  builder-pattern API for configuring processes and pipelines.  Once configured,
  [`start()`](https://docs.rs/subprocess/latest/subprocess/struct.Exec.html#method.start)
  starts the process or pipeline. Includes convenience methods like
  [`join()`](https://docs.rs/subprocess/latest/subprocess/struct.Exec.html#method.join)
  and
  [`capture()`](https://docs.rs/subprocess/latest/subprocess/struct.Exec.html#method.capture)
  that start, collect results, and wait for the process to finish in one call.

* **[`Job`](https://docs.rs/subprocess/latest/subprocess/struct.Job.html)** - interacts
  with a started process or pipeline, returned by
  [`start()`](https://docs.rs/subprocess/latest/subprocess/struct.Exec.html#method.start).
  It holds the pipe files (`stdin`, `stdout`, `stderr`) and has methods like `capture()`
  for interacting with them. It contains a list of `Process` handles and enables batch
  operations over processes like `wait()` and `terminate()`.

- **[`Process`](https://docs.rs/subprocess/latest/subprocess/struct.Process.html)** - a
  cheaply cloneable handle to a single running process. It provides `pid()`, `wait()`,
  `poll()`, `terminate()`, and `kill()`. Its methods take `&self`, so you can use them on
  `Process` shared across threads.

## Examples

### Execution

Execute a command and wait for it to complete:

```rust
Exec::cmd("umount").arg(dirname).checked().join()?;
```

`join()` starts the command and waits for it to finish, returning the exit
status. `checked()` ensures an error is returned for non-zero exit status.

To prevent quoting issues and shell injection attacks, `subprocess` doesn't spawn a shell
unless explicitly requested.  To execute a command through the OS shell, use
`Exec::shell`:

```rust
Exec::shell("shutdown -h now").join()?;
```

### Capturing output

Capture the stdout and stderr of a command, and use the stdout:

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

```
let top_mem = (Exec::cmd("ps").args(&["aux"])
    | Exec::cmd("sort").args(&["-k4", "-rn"])
    | Exec::cmd("head").arg("-5"))
    .capture()?
    .stdout_str();
```

Pipeline supports the same methods for interacting with the subprocess as with a single
started command.

### Streaming

Get stdout as an object that implements `std::io::Read` (like C's `popen`):

```rust
let stream = Exec::cmd("find").arg("/").stream_stdout()?;
// Use stream.read_to_string(), BufReader::new(stream).lines(), etc.
```

### Arbitrary input

`stdin()` doesn't accept just static strings, you can give it any owned data (such as in a
memory-mapped file or shared `bytes::Bytes` container), or generate it lazily:

```rust
use subprocess::InputData;

// send owned bytes
let bytes = bytes::Bytes::from("Hello world");
let gzipped_bytes = Exec::cmd("gzip")
    .stdin(InputData::from_bytes(bytes))
    .capture()?
    .stdout;

// send a gigabyte of zeros
let lazy_source = std::io::repeat(0).take(1_000_000_000);
let gzipped_stream = Exec::cmd("gzip")
    .stdin(InputData::from_reader(lazy_source))
    .stream_stdout()?;
```

The data is streamed to the subprocess in chunks.

### Timeout

Capture with timeout:

```rust
let response = Exec::cmd("curl").arg("-s").arg(url)
    .stdout(Redirection::Pipe)
    .start()?
    .capture_timeout(Duration::from_secs(10))?
    .stdout_str();
```

`communicate()` can be used for more sophisticated control over timeouts, such as reading
with a time or size limit:

```rust
let mut comm = Exec::cmd("ping").arg("example.com").communicate()?;
let (out, _) = comm
    .limit_time(Duration::from_secs(5))
    .read_string()?;
```

### Termination

Give the process some time to run, then terminate if needed:

```rust
let job = Exec::cmd("sleep").arg("10").start()?;
match job.wait_timeout(Duration::from_secs(1))? {
    Some(status) => println!("finished: {:?}", status),
    None => {
        job.terminate()?;
        job.wait()?;
    }
}
```

## License

`subprocess` is distributed under the terms of both the MIT license and the Apache License
(Version 2.0).  See [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT) for
details.  Contributing changes is assumed to signal agreement with these licensing terms.
