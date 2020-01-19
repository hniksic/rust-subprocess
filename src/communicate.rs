#[cfg(unix)]
mod os {
    use crate::posix;
    use std::cmp::min;
    use std::fs::File;
    use std::io::{Read, Result as IoResult, Write};
    use std::os::unix::io::AsRawFd;

    fn poll3(
        fin: Option<&File>,
        fout: Option<&File>,
        ferr: Option<&File>,
    ) -> IoResult<(bool, bool, bool)> {
        fn to_poll(f: Option<&File>, for_read: bool) -> posix::PollFd {
            let optfd = f.map(File::as_raw_fd);
            let events = if for_read {
                posix::POLLIN
            } else {
                posix::POLLOUT
            };
            posix::PollFd::new(optfd, events)
        }

        let mut fds = [
            to_poll(fin, false),
            to_poll(fout, true),
            to_poll(ferr, true),
        ];
        posix::poll(&mut fds, None)?;

        Ok((
            fds[0].test(posix::POLLOUT | posix::POLLHUP),
            fds[1].test(posix::POLLIN | posix::POLLHUP),
            fds[2].test(posix::POLLIN | posix::POLLHUP),
        ))
    }

    fn comm_poll(
        stdin_ref: &mut Option<File>,
        stdout_ref: &mut Option<File>,
        stderr_ref: &mut Option<File>,
        mut input_data: &[u8],
    ) -> IoResult<(Vec<u8>, Vec<u8>)> {
        // Note: chunk size for writing must be smaller than the pipe
        // buffer size.  A large enough write to a blocking deadlocks
        // despite the use of poll() to check that it's ok to write.
        const WRITE_SIZE: usize = 4096;

        let mut stdout_ref = stdout_ref.as_ref();
        let mut stderr_ref = stderr_ref.as_ref();

        let mut out = Vec::<u8>::new();
        let mut err = Vec::<u8>::new();

        loop {
            match (stdin_ref.as_ref(), stdout_ref, stderr_ref) {
                // When only a single stream remains for reading or
                // writing, we no longer need polling.  When no stream
                // remains, we are done.
                (Some(..), None, None) => {
                    stdin_ref.as_ref().unwrap().write_all(input_data)?;
                    // close stdin when done writing, so the child receives EOF
                    stdin_ref.take();
                    break;
                }
                (None, Some(ref mut stdout), None) => {
                    stdout.read_to_end(&mut out)?;
                    break;
                }
                (None, None, Some(ref mut stderr)) => {
                    stderr.read_to_end(&mut err)?;
                    break;
                }
                (None, None, None) => break,
                _ => (),
            }

            let (in_ready, out_ready, err_ready) =
                poll3(stdin_ref.as_ref(), stdout_ref, stderr_ref)?;
            if in_ready {
                let chunk = &input_data[..min(WRITE_SIZE, input_data.len())];
                let n = stdin_ref.as_ref().unwrap().write(chunk)?;
                input_data = &input_data[n..];
                if input_data.is_empty() {
                    // close stdin when done writing, so the child receives EOF
                    stdin_ref.take();
                }
            }
            if out_ready {
                let mut buf = [0u8; 4096];
                let n = stdout_ref.unwrap().read(&mut buf)?;
                if n != 0 {
                    out.extend(&buf[..n]);
                } else {
                    stdout_ref = None;
                }
            }
            if err_ready {
                let mut buf = [0u8; 4096];
                let n = stderr_ref.unwrap().read(&mut buf)?;
                if n != 0 {
                    err.extend(&buf[..n]);
                } else {
                    stderr_ref = None;
                }
            }
        }

        Ok((out, err))
    }

    pub fn communicate(
        stdin_ref: &mut Option<File>,
        stdout_ref: &mut Option<File>,
        stderr_ref: &mut Option<File>,
        input_data: Option<&[u8]>,
    ) -> IoResult<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        if stdin_ref.is_some() {
            input_data.expect("must provide input to redirected stdin");
        } else {
            assert!(
                input_data.is_none(),
                "cannot provide input to non-redirected stdin"
            );
        }
        let input_data = input_data.unwrap_or(b"");
        let (out, err) = comm_poll(stdin_ref, stdout_ref, stderr_ref, input_data)?;
        Ok((
            stdout_ref.as_ref().map(|_| out),
            stderr_ref.as_ref().map(|_| err),
        ))
    }
}

#[cfg(windows)]
mod os {
    use std::fs::File;
    use std::io::{Read, Result as IoResult, Write};

    fn comm_read(mut outfile: File) -> IoResult<Vec<u8>> {
        let mut contents = Vec::new();
        outfile.read_to_end(&mut contents)?;
        Ok(contents)
    }

    fn comm_write(mut infile: File, input_data: &[u8]) -> IoResult<()> {
        infile.write_all(input_data)?;
        Ok(())
    }

    // Call up to three functions in parallel, starting as many threads as
    // needed for the functions that are actually specified.
    pub fn parallel_call<R1, R2, R3>(
        f1: Option<impl FnOnce() -> R1 + Send>,
        f2: Option<impl FnOnce() -> R2 + Send>,
        f3: Option<impl FnOnce() -> R3 + Send>,
    ) -> (Option<R1>, Option<R2>, Option<R3>)
    where
        R1: Send,
        R2: Send,
        R3: Send,
    {
        match (f1, f2, f3) {
            // only create threads if necessary
            (None, None, None) => (None, None, None),
            (Some(f1), None, None) => (Some(f1()), None, None),
            (None, Some(f2), None) => (None, Some(f2()), None),
            (None, None, Some(f3)) => (None, None, Some(f3())),
            (f1, f2, f3) => crossbeam_utils::thread::scope(move |scope| {
                // run f2 and/or f3 in the background and let f1 run in our
                // thread
                let ta = f2.map(|f| scope.spawn(move |_| f()));
                let tb = f3.map(|f| scope.spawn(move |_| f()));
                (
                    f1.map(|f| f()),
                    ta.map(|t| t.join().unwrap()),
                    tb.map(|t| t.join().unwrap()),
                )
            })
            .unwrap(),
        }
    }

    pub fn communicate(
        stdin: &mut Option<File>,
        stdout: &mut Option<File>,
        stderr: &mut Option<File>,
        input_data: Option<&[u8]>,
    ) -> IoResult<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        let write_in_fn = stdin.take().map(|in_| {
            let input_data = input_data.expect("must provide input to redirected stdin");
            move || comm_write(in_, input_data)
        });
        if write_in_fn.is_none() && input_data.is_some() {
            panic!("cannot provide input to non-redirected stdin");
        }
        let read_out_fn = stdout.take().map(|out| || comm_read(out));
        let read_err_fn = stderr.take().map(|err| || comm_read(err));
        let (out, err, write_ret) = parallel_call(read_out_fn, read_err_fn, write_in_fn);
        if let Some(write_ret) = write_ret {
            let () = write_ret?;
        }
        Ok((
            if let Some(out) = out {
                Some(out?)
            } else {
                None
            },
            if let Some(err) = err {
                Some(err?)
            } else {
                None
            },
        ))
    }
}

pub use self::os::communicate;
