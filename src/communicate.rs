#[cfg(unix)]
mod os {
    use crate::posix;
    use std::cmp::min;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};

    fn millisecs_until(t: Instant) -> u32 {
        let now = Instant::now();
        if t <= now {
            return 0;
        }
        let diff = t - now;
        (diff.as_secs() * 1000) as u32 + diff.subsec_millis()
    }

    fn poll3(
        fin: Option<&File>,
        fout: Option<&File>,
        ferr: Option<&File>,
        deadline: Option<Instant>,
    ) -> io::Result<(bool, bool, bool)> {
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
        posix::poll(&mut fds, deadline.map(millisecs_until))?;

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
        deadline: Option<Instant>,
    ) -> io::Result<(Vec<u8>, Vec<u8>)> {
        // Note: chunk size for writing must be smaller than the pipe buffer
        // size.  A large enough write to a pipe deadlocks despite polling.
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
                    // take() to close stdin when done writing, so the child
                    // receives EOF
                    stdin_ref.take().unwrap().write_all(input_data)?;
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
                poll3(stdin_ref.as_ref(), stdout_ref, stderr_ref, deadline)?;
            if !in_ready && !out_ready && !err_ready {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "timeout"));
            }
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
        timeout: Option<Duration>,
    ) -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        if stdin_ref.is_some() {
            input_data.expect("must provide input to redirected stdin");
        } else {
            assert!(
                input_data.is_none(),
                "cannot provide input to non-redirected stdin"
            );
        }
        let input_data = input_data.unwrap_or(b"");
        let (out, err) = comm_poll(
            stdin_ref,
            stdout_ref,
            stderr_ref,
            input_data,
            timeout.map(|d| Instant::now() + d),
        )?;
        Ok((
            stdout_ref.as_ref().map(|_| out),
            stderr_ref.as_ref().map(|_| err),
        ))
    }
}

#[cfg(windows)]
mod os {
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::mem;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::thread;
    use std::time::{Duration, Instant};

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

    fn read_all(mut source: File) -> io::Result<Vec<u8>> {
        let mut out = vec![];
        source.read_to_end(&mut out)?;
        Ok(out)
    }

    pub fn communicate_sans_timeout(
        stdin: &mut Option<File>,
        stdout: &mut Option<File>,
        stderr: &mut Option<File>,
        input_data: Option<&[u8]>,
    ) -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        let read_out_fn = stdout.take().map(|out| || read_all(out));
        let read_err_fn = stderr.take().map(|err| || read_all(err));
        let write_in_fn = stdin.take().map(|mut in_| {
            let input_data = input_data.expect("must provide input to redirected stdin");
            move || in_.write_all(&input_data)
        });
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

    fn spawn_worker<T: Send + 'static>(
        active_workers: &mut u8,
        tx: T,
        f: impl FnOnce(T) + Send + 'static,
    ) -> thread::JoinHandle<()> {
        *active_workers += 1;
        thread::spawn(move || f(tx))
    }

    #[derive(Debug, Copy, Clone)]
    enum StreamIdent {
        In,
        Out,
        Err,
    }

    fn read_chunks(
        mut outfile: File,
        ident: StreamIdent,
        sink: mpsc::Sender<io::Result<(StreamIdent, Vec<u8>)>>,
    ) {
        let mut chunk = [0u8; 1024];
        loop {
            match outfile.read(&mut chunk) {
                Ok(nread) => {
                    if let Err(_) = sink.send(Ok((ident, chunk[..nread].to_vec()))) {
                        // sending will fail if the other worker reports a
                        // read error and the main thread gives up
                        break;
                    }
                    if nread == 0 {
                        break;
                    }
                }
                Err(e) => {
                    let _ = sink.send(Err(e));
                    break;
                }
            }
        }
    }

    pub fn communicate_with_timeout(
        stdin: &mut Option<File>,
        stdout: &mut Option<File>,
        stderr: &mut Option<File>,
        input_data: Option<&[u8]>,
        timeout: Duration,
    ) -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        let deadline = Instant::now() + timeout;

        let (mut outvec, mut errvec) = (None, None);
        let read_stdout = stdout.take().map(|outfile| {
            outvec = Some(vec![]);
            |tx| read_chunks(outfile, StreamIdent::Out, tx)
        });
        let read_stderr = stderr.take().map(|errfile| {
            errvec = Some(vec![]);
            |tx| read_chunks(errfile, StreamIdent::Err, tx)
        });
        let write_stdin = stdin.take().map(|mut in_| {
            // when using timeout we must make a copy of input_data
            // because its ownership must be kept by the threads
            let input_data = input_data
                .expect("must provide input to redirected stdin")
                .to_vec();
            move |tx: mpsc::Sender<_>| match in_.write_all(&input_data) {
                Ok(()) => mem::drop(tx.send(Ok((StreamIdent::In, vec![])))),
                Err(e) => mem::drop(tx.send(Err(e))),
            }
        });

        let mut active_cnt = 0u8;
        let (tx, rx) = mpsc::channel::<io::Result<(StreamIdent, Vec<u8>)>>();

        read_stdout.map(|f| spawn_worker(&mut active_cnt, tx.clone(), f));
        read_stderr.map(|f| spawn_worker(&mut active_cnt, tx.clone(), f));
        write_stdin.map(|f| spawn_worker(&mut active_cnt, tx.clone(), f));

        while active_cnt != 0 {
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "timeout"));
            }
            match rx.recv_timeout(deadline - now) {
                Ok(Ok((ident, data))) => {
                    match ident {
                        StreamIdent::Out => outvec.as_mut().unwrap().extend_from_slice(&data),
                        StreamIdent::Err => errvec.as_mut().unwrap().extend_from_slice(&data),
                        StreamIdent::In => (),
                    }
                    if data.len() == 0 {
                        active_cnt -= 1;
                    }
                }
                Ok(Err(e)) => return Err(e),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "timeout"))
                }
                // we should never be disconnected, as the threads must
                // announce that they're leaving (and because we hold a clone
                // of the sender)
                Err(RecvTimeoutError::Disconnected) => unreachable!(),
            }
        }

        Ok((outvec, errvec))
    }

    pub fn communicate(
        stdin: &mut Option<File>,
        stdout: &mut Option<File>,
        stderr: &mut Option<File>,
        input_data: Option<&[u8]>,
        timeout: Option<Duration>,
    ) -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        if stdin.is_none() && input_data.is_some() {
            panic!("cannot provide input to non-redirected stdin");
        }
        if let Some(timeout) = timeout {
            communicate_with_timeout(stdin, stdout, stderr, input_data, timeout)
        } else {
            communicate_sans_timeout(stdin, stdout, stderr, input_data)
        }
    }
}

pub use self::os::communicate;
