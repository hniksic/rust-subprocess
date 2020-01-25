use std::fs::File;
use std::io;
use std::time::{Duration, Instant};

#[cfg(unix)]
mod os {
    use crate::posix;
    use std::cmp::min;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::time::Instant;

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

    pub struct Communicator<'a> {
        stdin: Option<File>,
        stdout: Option<File>,
        stderr: Option<File>,
        input_data: &'a [u8],
    }

    impl<'a> Communicator<'a> {
        pub fn new(
            stdin: &mut Option<File>,
            stdout: &mut Option<File>,
            stderr: &mut Option<File>,
            input_data: Option<&'a [u8]>,
        ) -> Communicator<'a> {
            let input_data = input_data.unwrap_or(b"");
            Communicator {
                stdin: stdin.take(),
                stdout: stdout.take(),
                stderr: stderr.take(),
                input_data,
            }
        }

        pub fn communicate_until(
            &mut self,
            deadline: Option<Instant>,
        ) -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
            // Note: chunk size for writing must be smaller than the pipe buffer
            // size.  A large enough write to a pipe deadlocks despite polling.
            const WRITE_SIZE: usize = 4096;

            let mut stdout_ref = self.stdout.as_ref();
            let mut stderr_ref = self.stderr.as_ref();

            let mut out = Vec::<u8>::new();
            let mut err = Vec::<u8>::new();

            loop {
                match (self.stdin.as_ref(), stdout_ref, stderr_ref) {
                    // When only a single stream remains for reading or
                    // writing, we no longer need polling.  When no stream
                    // remains, we are done.
                    (Some(..), None, None) => {
                        // take() to close stdin when done writing, so the child
                        // receives EOF
                        self.stdin.take().unwrap().write_all(self.input_data)?;
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
                    poll3(self.stdin.as_ref(), stdout_ref, stderr_ref, deadline)?;
                if !in_ready && !out_ready && !err_ready {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "timeout"));
                }
                if in_ready {
                    let chunk = &self.input_data[..min(WRITE_SIZE, self.input_data.len())];
                    let n = self.stdin.as_ref().unwrap().write(chunk)?;
                    self.input_data = &self.input_data[n..];
                    if self.input_data.is_empty() {
                        // close stdin when done writing, so the child receives EOF
                        self.stdin.take();
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

            Ok((
                self.stdout.as_ref().map(|_| out),
                self.stderr.as_ref().map(|_| err),
            ))
        }
    }
}

#[cfg(windows)]
mod os {
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::marker::PhantomData;
    use std::mem;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::thread;
    use std::time::Instant;

    #[derive(Debug, Copy, Clone)]
    enum StreamIdent {
        In = 1 << 0,
        Out = 1 << 1,
        Err = 1 << 2,
    }

    fn read_chunks(
        mut outfile: File,
        ident: StreamIdent,
        sink: mpsc::SyncSender<io::Result<(StreamIdent, Vec<u8>)>>,
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

    // Although we store a copy of input data, use a lifetime for
    // compatibility with the more efficient Unix version.
    pub struct Communicator<'a> {
        rx: mpsc::Receiver<io::Result<(StreamIdent, Vec<u8>)>>,
        worker_set: u8,
        marker: PhantomData<&'a u8>,
    }

    impl<'a> Communicator<'a> {
        pub fn new(
            stdin: &mut Option<File>,
            stdout: &mut Option<File>,
            stderr: &mut Option<File>,
            input_data: Option<&[u8]>,
        ) -> Communicator<'a> {
            let mut worker_set = 0u8;

            let read_stdout = stdout.take().map(|outfile| {
                worker_set |= StreamIdent::Out as u8;
                |tx| read_chunks(outfile, StreamIdent::Out, tx)
            });
            let read_stderr = stderr.take().map(|errfile| {
                worker_set |= StreamIdent::Err as u8;
                |tx| read_chunks(errfile, StreamIdent::Err, tx)
            });
            let write_stdin = stdin.take().map(|mut in_| {
                // when using timeout we must make a copy of input_data
                // because its ownership must be kept by the threads
                let input_data = input_data
                    .expect("must provide input to redirected stdin")
                    .to_vec();
                worker_set |= StreamIdent::In as u8;
                move |tx: mpsc::SyncSender<_>| match in_.write_all(&input_data) {
                    Ok(()) => mem::drop(tx.send(Ok((StreamIdent::In, vec![])))),
                    Err(e) => mem::drop(tx.send(Err(e))),
                }
            });

            let (tx, rx) = mpsc::sync_channel(1);

            type Sender = mpsc::SyncSender<io::Result<(StreamIdent, Vec<u8>)>>;
            fn spawn_worker(tx: Sender, f: impl FnOnce(Sender) + Send + 'static) {
                thread::spawn(move || f(tx));
            }

            read_stdout.map(|f| spawn_worker(tx.clone(), f));
            read_stderr.map(|f| spawn_worker(tx.clone(), f));
            write_stdin.map(|f| spawn_worker(tx.clone(), f));

            Communicator {
                rx,
                worker_set,
                marker: PhantomData,
            }
        }

        fn recv_until(
            &self,
            deadline: Option<Instant>,
        ) -> Option<io::Result<(StreamIdent, Vec<u8>)>> {
            if let Some(deadline) = deadline {
                let now = Instant::now();
                if now >= deadline {
                    return None;
                }
                match self.rx.recv_timeout(deadline - now) {
                    Ok(result) => Some(result),
                    Err(RecvTimeoutError::Timeout) => None,
                    // we should never be disconnected, as the threads must
                    // announce that they're leaving
                    Err(RecvTimeoutError::Disconnected) => unreachable!(),
                }
            } else {
                Some(self.rx.recv().unwrap())
            }
        }

        pub fn communicate_until(
            &mut self,
            deadline: Option<Instant>,
        ) -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
            let (mut outvec, mut errvec) = (None, None);

            if self.worker_set & StreamIdent::Out as u8 != 0 {
                outvec = Some(vec![]);
            }
            if self.worker_set & StreamIdent::Err as u8 != 0 {
                errvec = Some(vec![]);
            }

            while self.worker_set != 0 {
                match self.recv_until(deadline) {
                    Some(Ok((ident, data))) => {
                        match ident {
                            StreamIdent::Out => outvec.as_mut().unwrap().extend_from_slice(&data),
                            StreamIdent::Err => errvec.as_mut().unwrap().extend_from_slice(&data),
                            StreamIdent::In => (),
                        }
                        if data.len() == 0 {
                            self.worker_set &= !(ident as u8);
                        }
                    }
                    Some(Err(e)) => return Err(e),
                    None => return Err(io::Error::new(io::ErrorKind::Interrupted, "timeout")),
                }
            }

            Ok((outvec, errvec))
        }
    }
}

pub struct Communicator<'a> {
    inner: os::Communicator<'a>,
}

impl Communicator<'_> {
    pub fn communicate_until(
        &mut self,
        deadline: Option<Instant>,
    ) -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        self.inner.communicate_until(deadline)
    }

    pub fn communicate_timeout(
        &mut self,
        timeout: Duration,
    ) -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        self.inner.communicate_until(Some(Instant::now() + timeout))
    }
}

pub fn communicate<'a>(
    stdin: &mut Option<File>,
    stdout: &mut Option<File>,
    stderr: &mut Option<File>,
    input_data: Option<&'a [u8]>,
) -> Communicator<'a> {
    if stdin.is_some() {
        input_data.expect("must provide input to redirected stdin");
    } else {
        assert!(
            input_data.is_none(),
            "cannot provide input to non-redirected stdin"
        );
    }
    Communicator {
        inner: os::Communicator::new(stdin, stdout, stderr, input_data),
    }
}
