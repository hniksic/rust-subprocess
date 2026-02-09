# Migrating from subprocess 0.2 to 1.0

subprocess 1.0 is an incompatible change to the public API of the crate. The core
capabilities are the same - spawning processes, building pipelines, deadlock-free
capture - but the release cleans up long-standing API warts accumulated over the years.

This guide covers the breaking changes and shows how to update your code.

## What's changed

Before diving into the migration details, here is a summary of changes:

- **Popen type is removed.** `Popen` mixed process creation, pipe ownership, and awaiting
  in one type. Setup is now done through the builder API, which was already
  available. `Job` controls pipe access and operations over spawned processes as a unit.

- **Simplified errors.** Everything returns `std::io::Error`, reflecting the fact that
  virtually all errors in the library come from failed syscalls.

- **Richer pipeline support.** Pipelines gain almost all methods of `Exec`, making
  pipeline creation and control equally convenient as that of single commands.
  Single-command and empty pipelines are now allowed, simplifying dynamic pipeline
  construction.

- **Exit status checking.** The new `checked()` method on `Exec` and
  `Pipeline` makes it a one-liner to treat non-zero exits as errors.
  
- **No-allocation input.** Process input provided from Rust no longer requires collecting
  into a `Vec<u8>`. `stdin()` now directly accepts owned data like `bytes::Bytes`,
  `memmap2::Mmap`, static data like `&'static str`, or even an arbitrary `impl
  std::io::Read`, never collecting the data.

## Error types

Custom error types `PopenError` and `CommunicateError` have been removed. All fallible
operations return `std::io::Error`.

```rust
// 0.2
use subprocess::{PopenError, Result};
fn run() -> subprocess::Result<()> { /* ... */ }

// 1.0
fn run() -> std::io::Result<()> { /* ... */ }
```

## Popen and PopenConfig

The biggest change in 1.0 is the removal of `Popen` and `PopenConfig`.

In 0.2, `Popen` was the central underlying type: it controlled process creation, held the
process handle, owned the pipe file descriptors, and provided the wait/communicate API. In
1.0, these responsibilities are separated:

- **`Process`** is a lightweight, cheaply cloneable handle to a single running process. It
  provides `pid()`, `wait()`, `poll()`, `terminate()`, and `kill()`. Its methods take
  `&self`, not `&mut self`, so you can share a `Process` across threads without a mutex.

- **`Job`** is returned by `Exec::start()` and `Pipeline::start()`. It holds the pipe file
  descriptors (`stdin`, `stdout`, `stderr`), the list of `Process` handles, and provides
  batch operations like `wait()`, `terminate()`, and `kill()` over all processes. It also
  provides the lifecycle methods `join()`, `capture()`, and `communicate()`.

`PopenConfig` is replaced by (previously present) builder methods on `Exec`:

```rust
// 0.2
let p = Popen::create(&["ls", "-la"], PopenConfig {
    stdout: Redirection::Pipe,
    stderr: Redirection::Merge,
    cwd: Some("/tmp".into()),
    env: Some(vec![("KEY".into(), "val".into())]),
    detached: true,
    ..Default::default()
})?;

// 1.0
let job = Exec::cmd("ls").arg("-la")
    .stdout(Redirection::Pipe)
    .stderr(Redirection::Merge)
    .cwd("/tmp")
    .env("KEY", "val")
    .detached()
    .start()?;
```

If you used `Popen` for low-level pipe access:

```rust
// 0.2 - popen()
let mut p = Exec::cmd("cmd").stdout(Redirection::Pipe).popen()?;
let mut buf = String::new();
p.stdout.as_mut().unwrap().read_to_string(&mut buf)?;
p.wait()?;

// 1.0 - start()
let mut job = Exec::cmd("cmd").stdout(Redirection::Pipe).start()?;
let mut buf = String::new();
job.stdout.as_mut().unwrap().read_to_string(&mut buf)?;
job.wait()?;
```

For many common cases, the high-level methods on `Exec` still work the same way:

```rust
// Works in both 0.2 and 1.0
let output = Exec::cmd("cmd").stream_stdout()?;
let capture = Exec::cmd("cmd").capture()?;
let status = Exec::cmd("cmd").join()?;
```

## ExitStatus

`ExitStatus` was a four-variant enum exposing platform details. It's now an opaque newtype
with accessor methods, matching the conventions of `std::process::ExitStatus`.

## Pipeline changes

### Creation

`Pipeline::new()` no longer requires exactly two commands, and
`Pipeline::from_exec_iter()` is replaced by `FromIterator`:

```rust
// 0.2
let p = Pipeline::from_exec_iter(commands);        // panics if < 2 commands

// 1.0
let p: Pipeline = commands.into_iter().collect();  // works any number of commands
```

The `|` operator still works as before:

```rust
let p = Exec::cmd("find") | Exec::cmd("sort") | Exec::cmd("uniq");
```

You can now additionally write:

```rust
let p = Pipeline::new()
    .pipe(Exec::cmd("find"))
    .pipe(Exec::cmd("sort"))
    .pipe(Exec::cmd("uniq"));
```

### Starting

`Pipeline::popen()` returned `Vec<Popen>`. The replacement is `Pipeline::start()`, which
returns a `Job`:

```rust
// 0.2
let popens: Vec<Popen> = pipeline.popen()?;

// 1.0
let job: Job = pipeline.start()?;
// job.processes is a Vec<Process>
```

### stderr redirection

`Pipeline::stderr_to()` only accepted a `File`. The new
`Pipeline::stderr_all()` accepts any `OutputRedirection`:

```rust
// 0.2
pipeline.stderr_to(file)

// 1.0
pipeline.stderr_all(file)
pipeline.stderr_all(Redirection::Null)
pipeline.stderr_all(Redirection::Pipe)
```

## NullFile

The `NullFile` marker struct is replaced by `Redirection::Null`:

```rust
// 0.2
use subprocess::NullFile;
Exec::cmd("noisy").stdout(NullFile).stderr(NullFile);

// 1.0
Exec::cmd("noisy").stdout(Redirection::Null).stderr(Redirection::Null);
```

## Timeouts

In 0.2, timeouts were configured on the builder with `time_limit()`. In 1.0, they live on
`Job`, where you configure what to run separately from how long to wait:

```rust
// 0.2
let capture = Exec::cmd("slow")
    .time_limit(Duration::from_secs(5))
    .capture()?;

// 1.0
let capture = Exec::cmd("slow")
    .stdout(Redirection::Pipe)
    .start()?
    .capture_timeout(Duration::from_secs(5))?;
```

## Exit status checking

1.0 adds `checked()` to `Exec` and `Pipeline`. When set, terminator methods return an
error if the process exits with a non-zero status:

```rust
// 1.0
Exec::cmd("false").checked().join()?;       // returns Err
(cmd1 | cmd2).checked().capture()?;         // Err if last cmd fails

// Also available on Capture:
let cap = Exec::cmd("maybe-fails").capture()?.check()?;
```

## Communicator

`Communicator::read()` no longer wraps results in `Option` - for non-set-up redirections
output will simply be empty:

```rust
// 0.2
let (stdout, stderr) = comm.read()?;
// stdout: Option<Vec<u8>>, stderr: Option<Vec<u8>>

// 1.0
let (stdout, stderr) = comm.read()?;
// stdout: Vec<u8>, stderr: Vec<u8>
```

The new `read_to()` method lets you direct output to arbitrary writers without buffering:

```rust
// 1.0
comm.read_to(&mut stdout_sink, &mut stderr_sink)?;
```

Creating a communicator from `Exec` is simpler too:

```rust
// 0.2
let mut p = Exec::cmd("cat").stdin("input").popen()?;
let comm = p.communicate_start(Some(b"data".to_vec()));

// 1.0
let mut comm = Exec::cmd("cat").stdin("input").communicate()?;
```

## Stdin data

Passing data to stdin is more flexible. The following are supported:

```rust
Exec::cmd("cat").stdin("hello");                        // &str
Exec::cmd("cat").stdin(b"hello".as_slice());            // &[u8]
Exec::cmd("cat").stdin(vec![1, 2, 3]);                  // Vec<u8>
Exec::cmd("cat").stdin(b"hello");                       // &[u8; N]
Exec::cmd("cat").stdin(InputData::from_bytes(bytes));   // any impl AsRef<[u8]>
Exec::cmd("cat").stdin(InputData::from_reader(source)); // any impl std::io::Read
```

All of these take ownership of the data without additional allocations.

## Exec and Pipeline are no longer Clone

In 0.2, `Exec` and `Pipeline` implemented `Clone` with the implementation panicking in
case of error. In 1.0, they no longer implement `Clone`.

## &mut self -> &self on Process

`Process` methods take `&self` instead of `&mut self`, and `Process` is cheaply
cloneable. This means you can share a process handle across threads or keep multiple
references without a mutex:

```rust
let job = Exec::cmd("server").start()?;
let process = job.processes[0].clone();

// In another thread:
process.wait()?;
```

## Quick reference

| 0.2 | 1.0 |
|-----|-----|
| `Popen` | `Process` + `Job` |
| `PopenConfig { ... }` | builder methods on `Exec` |
| `PopenError` | `std::io::Error` |
| `subprocess::Result<T>` | `std::io::Result<T>` |
| `CommunicateError` | `std::io::Error` |
| `CaptureData` | `Capture` |
| `NullFile` | `Redirection::Null` |
| `ExitStatus::Exited(n)` | `status.code()` |
| `ExitStatus::Signaled(n)` | `status.signal()` |
| `Exec::popen()` | `Exec::start()` -> `Job` |
| `Pipeline::popen()` | `Pipeline::start()` -> `Job` |
| `Pipeline::from_exec_iter(v)` | `v.into_iter().collect()` |
| `Pipeline::stderr_to(file)` | `Pipeline::stderr_all(file)` |
| `Exec::time_limit(d)` | `job.join_timeout(d)` / `job.capture_timeout(d)` |
| `p.communicate_start(data)` | `exec.communicate()?` |
