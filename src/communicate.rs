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
            stdin: Option<File>,
            stdout: Option<File>,
            stderr: Option<File>,
            input_data: Option<&'a [u8]>,
        ) -> Communicator<'a> {
            let input_data = input_data.unwrap_or(b"");
            Communicator {
                stdin,
                stdout,
                stderr,
                input_data,
            }
        }

        fn do_read(
            source_ref: &mut Option<&File>,
            dest: &mut Vec<u8>,
            size_limit: Option<usize>,
            total_read: usize,
        ) -> io::Result<()> {
            let mut buf = &mut [0u8; 4096][..];
            if let Some(size_limit) = size_limit {
                if total_read >= size_limit {
                    return Ok(());
                }
                if size_limit - total_read < buf.len() {
                    buf = &mut buf[0..size_limit - total_read];
                }
            }
            let n = source_ref.unwrap().read(buf)?;
            if n != 0 {
                dest.extend_from_slice(&mut buf[..n]);
            } else {
                *source_ref = None;
            }
            Ok(())
        }

        pub fn read(
            &mut self,
            deadline: Option<Instant>,
            size_limit: Option<usize>,
        ) -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
            // Note: chunk size for writing must be smaller than the pipe buffer
            // size.  A large enough write to a pipe deadlocks despite polling.
            const WRITE_SIZE: usize = 4096;

            let mut stdout_ref = self.stdout.as_ref();
            let mut stderr_ref = self.stderr.as_ref();

            let mut out = Vec::<u8>::new();
            let mut err = Vec::<u8>::new();

            loop {
                if let Some(size_limit) = size_limit {
                    if out.len() + err.len() >= size_limit {
                        break;
                    }
                }

                if let (None, None, None) = (self.stdin.as_ref(), stdout_ref, stderr_ref) {
                    // When no stream remains, we are done.
                    break;
                }

                let (in_ready, out_ready, err_ready) =
                    poll3(self.stdin.as_ref(), stdout_ref, stderr_ref, deadline)?;
                if !in_ready && !out_ready && !err_ready {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "timeout"));
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
                    let total = out.len() + err.len();
                    Communicator::do_read(&mut stdout_ref, &mut out, size_limit, total)?;
                }
                if err_ready {
                    let total = out.len() + err.len();
                    Communicator::do_read(&mut stderr_ref, &mut err, size_limit, total)?;
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
    use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
    use std::thread;
    use std::time::Instant;

    #[derive(Debug, Copy, Clone)]
    enum StreamIdent {
        In = 1 << 0,
        Out = 1 << 1,
        Err = 1 << 2,
    }

    enum Payload {
        Data(Vec<u8>),
        EOF,
        Err(io::Error),
    }

    // Messages exchanged between Communicator's helper threads.
    type Message = (StreamIdent, Payload);

    fn read_and_transmit(mut outfile: File, ident: StreamIdent, sink: SyncSender<Message>) {
        let mut chunk = [0u8; 4096];
        // Note: failing to sending to the sink means we are done.  It will
        // fail if the main thread drops the Communicator (and with it the
        // receiver) prematurely e.g. because a limit was reached or another
        // helper encountered an IO error.
        loop {
            match outfile.read(&mut chunk) {
                Ok(0) => {
                    let _ = sink.send((ident, Payload::EOF));
                    break;
                }
                Ok(nread) => {
                    if let Err(_) = sink.send((ident, Payload::Data(chunk[..nread].to_vec()))) {
                        break;
                    }
                }
                Err(e) => {
                    let _ = sink.send((ident, Payload::Err(e)));
                    break;
                }
            }
        }
    }

    fn spawn_curried<T: Send + 'static>(f: impl FnOnce(T) + Send + 'static, arg: T) {
        thread::spawn(move || f(arg));
    }

    // Although we store a copy of input data, use a lifetime for
    // compatibility with the more efficient Unix version.
    pub struct Communicator<'a> {
        rx: mpsc::Receiver<Message>,
        helper_set: u8,
        requested_streams: u8,
        leftover: Option<(StreamIdent, Vec<u8>)>,
        marker: PhantomData<&'a u8>,
    }

    struct Timeout;

    impl<'a> Communicator<'a> {
        pub fn new(
            stdin: Option<File>,
            stdout: Option<File>,
            stderr: Option<File>,
            input_data: Option<&[u8]>,
        ) -> Communicator<'a> {
            let mut helper_set = 0u8;
            let mut requested_streams = 0u8;

            let read_stdout = stdout.map(|stdout| {
                helper_set |= StreamIdent::Out as u8;
                requested_streams |= StreamIdent::Out as u8;
                |tx| read_and_transmit(stdout, StreamIdent::Out, tx)
            });
            let read_stderr = stderr.map(|stderr| {
                helper_set |= StreamIdent::Err as u8;
                requested_streams |= StreamIdent::Err as u8;
                |tx| read_and_transmit(stderr, StreamIdent::Err, tx)
            });
            let write_stdin = stdin.map(|mut stdin| {
                // when using timeout we must make a copy of input_data
                // because its ownership must be kept by the threads
                let input_data = input_data
                    .expect("must provide input to redirected stdin")
                    .to_vec();
                helper_set |= StreamIdent::In as u8;
                move |tx: SyncSender<_>| match stdin.write_all(&input_data) {
                    Ok(()) => mem::drop(tx.send((StreamIdent::In, Payload::EOF))),
                    Err(e) => mem::drop(tx.send((StreamIdent::In, Payload::Err(e)))),
                }
            });

            let (tx, rx) = mpsc::sync_channel(1);

            read_stdout.map(|f| spawn_curried(f, tx.clone()));
            read_stderr.map(|f| spawn_curried(f, tx.clone()));
            write_stdin.map(|f| spawn_curried(f, tx.clone()));

            Communicator {
                rx,
                helper_set,
                requested_streams,
                leftover: None,
                marker: PhantomData,
            }
        }

        fn recv_until(&self, deadline: Option<Instant>) -> Result<Message, Timeout> {
            if let Some(deadline) = deadline {
                let now = Instant::now();
                if now >= deadline {
                    return Err(Timeout);
                }
                match self.rx.recv_timeout(deadline - now) {
                    Ok(message) => Ok(message),
                    Err(RecvTimeoutError::Timeout) => Err(Timeout),
                    // should never be disconnected, the helper threads always
                    // announce their exit
                    Err(RecvTimeoutError::Disconnected) => unreachable!(),
                }
            } else {
                Ok(self.rx.recv().unwrap())
            }
        }

        fn as_options(
            &self,
            outvec: Vec<u8>,
            errvec: Vec<u8>,
        ) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
            let (mut o, mut e) = (None, None);
            if self.requested_streams & StreamIdent::Out as u8 != 0 {
                o = Some(outvec);
            } else {
                assert!(outvec.len() == 0);
            }
            if self.requested_streams & StreamIdent::Err as u8 != 0 {
                e = Some(errvec);
            } else {
                assert!(errvec.len() == 0);
            }
            (o, e)
        }

        pub fn read(
            &mut self,
            deadline: Option<Instant>,
            size_limit: Option<usize>,
        ) -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
            // Create both vectors immediately.  This doesn't allocate, and if
            // one of those is not needed, it just won't get resized.
            let mut outvec = vec![];
            let mut errvec = vec![];

            let mut grow_result =
                |ident, mut data: &[u8], leftover: &mut Option<(StreamIdent, Vec<u8>)>| {
                    if let Some(size_limit) = size_limit {
                        let total_read = outvec.len() + errvec.len();
                        if total_read >= size_limit {
                            return false;
                        }
                        let remaining = size_limit - total_read;
                        if data.len() > remaining {
                            *leftover = Some((ident, data[remaining..].to_vec()));
                            data = &data[..remaining];
                        }
                    }
                    let destvec = match ident {
                        StreamIdent::Out => &mut outvec,
                        StreamIdent::Err => &mut errvec,
                        StreamIdent::In => unreachable!(),
                    };
                    destvec.extend_from_slice(data);
                    if let Some(size_limit) = size_limit {
                        if outvec.len() + errvec.len() >= size_limit {
                            return false;
                        }
                    }
                    return true;
                };

            if let Some((ident, data)) = self.leftover.take() {
                if !grow_result(ident, &data, &mut self.leftover) {
                    return Ok(self.as_options(outvec, errvec));
                }
            }

            while self.helper_set != 0 {
                match self.recv_until(deadline) {
                    Ok((ident, Payload::EOF)) => {
                        self.helper_set &= !(ident as u8);
                        continue;
                    }
                    Ok((ident, Payload::Data(data))) => {
                        assert!(data.len() != 0);
                        if !grow_result(ident, &data, &mut self.leftover) {
                            break;
                        }
                    }
                    Ok((_ident, Payload::Err(e))) => return Err(e),
                    Err(Timeout) => return Err(io::Error::new(io::ErrorKind::TimedOut, "timeout")),
                }
            }

            Ok(self.as_options(outvec, errvec))
        }
    }
}

pub struct Communicator<'a> {
    inner: os::Communicator<'a>,
    read_size_limit: Option<usize>,
    read_time_limit: Option<Duration>,
}

impl<'a> Communicator<'a> {
    pub fn new(
        stdin: Option<File>,
        stdout: Option<File>,
        stderr: Option<File>,
        input_data: Option<&[u8]>,
    ) -> Communicator {
        Communicator {
            inner: os::Communicator::new(stdin, stdout, stderr, input_data),
            read_size_limit: None,
            read_time_limit: None,
        }
    }

    pub fn read(&mut self) -> io::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        let deadline = self
            .read_time_limit
            .map(|timeout| Instant::now() + timeout);
        self.inner.read(deadline, self.read_size_limit)
    }

    pub fn limit_size(mut self, size: usize) -> Communicator<'a> {
        self.read_size_limit = Some(size);
        self
    }

    pub fn limit_time(mut self, time: Duration) -> Communicator<'a> {
        self.read_time_limit = Some(time);
        self
    }
}

pub fn communicate<'a>(
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
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
    Communicator::new(stdin, stdout, stderr, input_data)
}
