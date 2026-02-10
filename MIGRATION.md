# Migrating from subprocess 0.2 to 1.0

subprocess 1.0 is an incompatible change to the public API of the crate. The capabilities
are the same - spawning processes, building pipelines, deadlock-free capture - but the
release cleans up long-standing API warts accumulated over the years.

This guide covers the changes and shows how to update your code.

## What's changed

Before diving into the migration details, here is a summary of changes.

### Popen type removed

`Popen` type, inspired by Python's `subprocess.Popen`, served as the backbone of this
crate for years. But soon after introduction of the builder API, it turned out that it's
much more convenient to use - because it allows building pipelines, among other things.
In theory `Popen` remained as a way to get low-level control over things, but in practice
it just duplicated functionality of builder.

Also, it mixed process creation, pipe ownership, and awaiting in one type, and the type
was not suitable for use in pipelines - which is why `Pipeline::popen()` was returning
`Vec<Popen>`, with the first and the last element having useful pipes.

All setup is now done through builders, and `Job` is in charge of pipes and operations
over spawned processes, regardless of whether it's a single process or a pipeline.

### Simplified errors

The `PopenError` type only had two variants, one for IO error and the other for logic
errors, and the other one was rarely used. The library now consistently uses
`std::io::Error` for errors, reflecting the fact that virtually all errors in the
library come from failed syscalls. When a "logic error" is detected, it is signaled as
`ErrorKind::InvalidInput`. `CommunicateError` was likewise removed.

Many entry points that previously panicked have been converted to return errors instead,
increasing robustness of the library.

### Richer pipeline support

Pipeline setup gain almost all methods of `Exec`, and starting them returns the same `Job`
type returned by starting a single command. This makes control of pipeline-backed jobs
equally powerful and convenient as that of single commands.  Single-command and empty
pipelines are now allowed, simplifying dynamic pipeline construction.

### Exit status checking

The new `checked()` method on `Exec` and `Pipeline` makes it a one-liner to treat non-zero
exits as errors. This has **not** been made default, however (unlike duct), because many
of the use cases of the library are about capturing errors, and automatic returning of
`Err(std::io::Error)` would be in the way of that.

### Non-collecting input

It is now possible to provide input to `stdin` that is held in memory or generated,
without the library collecting it into a `Vec<u8>`. This enables you to send the contents
of a `Vec<u8>`, `&'static str`, `bytes::Bytes`, or `memmap2::Mmap` to a function's stdin
without unnecessary copies or extra allocations.

In addition to that, you can now lazily generate input by passing any `impl Read` to the
subprocess.  E.g. `stdin(InputData::from_reader(std::io::repeat(0).take(1_000_000_000)))`
will send a gigabyte of zeros to the subprocess without spending a gigabyte of memory.

## Migration guide

### `Popen` and `PopenConfig`

`PopenConfig` is replaced by (already present) builder methods on `Exec`:

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

If you used `Popen` for low-level pipe access, get the same pipes from `Job`:

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

### ExitStatus

`ExitStatus` was a four-variant enum exposing platform details. It's now an opaque newtype
with accessor methods, matching the conventions of `std::process::ExitStatus`.

### Pipeline changes

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

`Pipeline::stderr_to()` only accepted a `File`. The new `Pipeline::stderr_all()` accepts
any `OutputRedirection`:

```rust
// 0.2
pipeline.stderr_to(file)

// 1.0
pipeline.stderr_all(file)
pipeline.stderr_all(Redirection::Null)
pipeline.stderr_all(Redirection::Pipe)
```

Like `stderr_to` before it, `stderr_all()` redirects output of **all** commands in the
pipeline to the specified sink. If you want to only redirect the last command, you can do
it on a per-`Exec` basis, as before.

### NullFile

The `NullFile` marker struct is replaced by `Redirection::Null`:

```rust
// 0.2
use subprocess::NullFile;
Exec::cmd("noisy").stdout(NullFile).stderr(NullFile);

// 1.0
Exec::cmd("noisy").stdout(Redirection::Null).stderr(Redirection::Null);
```

### Timeouts

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

### Communicator

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
