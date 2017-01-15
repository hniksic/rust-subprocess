# subprocess

The `subprocess` library provides facilities for execution of and
interaction with external processes and pipelines.  It is inspired by
Python's `subprocess` module, adding a Rust spin on top.

## Features

The following features are available:

* Launching external processes with optional redirection of standard
  input, output, and error.

* Waiting for the process to finish and polling its status: `poll`,
  `wait`, and `wait_timeout`.

* Advanced redirection options, such as connecting standard streams to
  arbitrary files, or merging errors into output like shell's `2>&1`
  operator.

* The `communicate` method for deadlock-free reading of output while
  simultaneously providing input to the subprocess.

* Connecting multiple commands into OS-level pipelines.

* Rustic builder-style API for building commands and pipelines and
  collecting their results.

The crate has minimal dependencies to third-party crates, only
requiring `libc`, `crossbeam`, and Win32-related crates on Windows.
It is intended to work on Unix-like platforms as well as on recent
Windows 7 and later.  It is regularly tested on Linux and Windows, and
occasionally on FreeBSD and MacOS.

## API Overview

The API is separated in two parts: the low-level `Popen` API directly
inspired by Python's `subprocess.Popen`, and the higher-level
convenience API using the builder pattern, in the vein of
`std::process`.  The two can be mixed, so it is possible to use
builder to create `Popen` instances, and then to continue working with
them directly.

The `Popen` type offers some functionality currently missing from the
Rust standard library.  It provides methods for polling the process,
waiting with timeout, and the `communicate` method, useful enough to
have been [created
independently](https://crates.io/crates/subprocess-communicate).
While the design follows Python's [`subprocess`
module](https://docs.python.org/3/library/subprocess.html#popen-constructor),
this module was adapted to Rust's style.  Some of the changes
accommodate the differences between the languages, such as the lack of
default and keyword arguments in Rust, and others take advantage of
Rust's more advanced type system, or of additional capabilities such
as the ownership system and the `Drop` trait.  Python's utility
functions such as `subprocess.run` are not included because they have
better alternatives in the form of the builder API.

Working with subprocesses using `subprocess::Popen` can look like
this:

```rust
let mut p = Popen::create(&["command", "arg1", "arg2"], PopenConfig {
    stdout: Redirection::Pipe, ..Default::default()
})?;

// Since we requested stdout to be redirected to a pipe, the parent's
// end of the pipe is available as p.stdout.  It can either be read
// directly, or processed using the communicate() method:
let (out, err) = p.communicate(None)?;

// check if the process is still alive
if let Some(exit_status) = p.poll() {
  // the process has finished
} else {
  // it is still running, terminate it
  p.terminate()?;
}
```

The builder API offers a more Rustic process creation interface, along
with convenience methods for capturing output and building pipelines.

```rust
let output = Exec::cmd("command").arg("arg1").arg("arg2")
    .stdout(Redirection::Pipe)
    .capture()?
    .stdout_str();
```

## Examples

Note: these examples use the `?` operator to show the result.  If you
are pasting them in a function that doesn't return a `Result`, such as
`main`, simply replace `X?` with `X.expect("failed to execute")`, or
with an appropriate pattern match.

### Commands

Execute an external command and wait for it to complete:

```rust
let exit_status = Exec::cmd("umount").arg(dirname).join()?;
```

Execute the command using the OS shell, like C's `system`:

```rust
Exec::shell("shutdown -h now").join()?;
```

Start a subprocess and obtain its output as a `Read` trait object,
like C's `popen`:

```rust
let stream = Exec::cmd("ls").stream_stdout()?;
// call stream.read_to_string, construct io::BufReader(stream), etc.
```

Capture the output of a command:

```rust
let out = Exec::cmd("ls")
  .stdout(Redirection::Pipe)
  .capture()?
  .stdout_str();
```

Redirect errors to standard output, and capture both in a single stream:

```rust
let out_and_err = Exec::cmd("ls")
  .stdout(Redirection::Pipe)
  .stderr(Redirection::Merge)
  .capture()?
  .stdout_str();
```

Provide input to the command and read its output:

```rust
let out = Exec::cmd("sort")
  .stdin("b\nc\na\n")
  .stdout(Redirection::Pipe)
  .capture()?
  .stdout_str();
assert!(out == "a\nb\nc\n");
```

Connecting `stdin` to an open file would have worked as well.

### Pipelines

`Popen` objects support connecting input and output to arbitrary open
files, including other `Popen` objects.  This can be used to form
pipelines of processes.  The builder API will do it automatically with
the `|` operator on `Exec` objects.

Just like in a Unix shell, each command in a pipeline receives
standard input from the previous command, and passes standard output
to the next command in the pipeline.  Optionally, the standard input
of the first command can be provided from the outside, and the output
of the last command can be captured.

Execute a pipeline and return the exit status of the last command:

```rust
let exit_status =
  (Exec::shell("ls *.bak") | Exec::cmd("xargs").arg("rm")).join()?;
```

Capture the pipeline's output:

```rust
let dir_checksum = {
    Exec::shell("find . -type f") | Exec::cmd("sort") | Exec::cmd("sha1sum")
}.capture()?.stdout_str();
```

### Interacting with subprocess

Check whether a previously launched process is still running:

```rust
let mut p = Exec::cmd("sleep").arg("2").popen()?;
thread::sleep(Duration::new(1, 0))
if p.poll().is_none() {
    // poll() returns Some(exit_status) if the process has completed
    println!("process is still running");
}
```

Give the process 1 second to run, and kill it if it didn't complete by
then.

```rust
let mut p = Exec::cmd("sleep").arg("2").popen()?;
if let Some(status) = p.wait_timeout(Duration::new(1, 0))? {
    println!("process finished as {:?}", status);
} else {
    p.kill()?;
    p.wait()?;
    println!("process killed");
}
```

## License

`subprocess` is distributed under the terms of both the MIT license
and the Apache License (Version 2.0).  See
[LICENSE-APACHE](LICENSE-APACHE) for [LICENSE-MIT](LICENSE-MIT) for
details.  Contributing changes is assumed to signal agreement with
these licensing terms.
