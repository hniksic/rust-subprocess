#[cfg(unix)]
mod os {
    use crate::posix;
    use std::fs::File;
    use std::io::{Read, Write, Result as IoResult};
    use std::os::unix::io::AsRawFd;
    use std::cmp::min;

    fn poll3(fin: Option<&File>, fout: Option<&File>, ferr: Option<&File>)
             -> IoResult<(bool, bool, bool)> {
        fn to_poll(f: Option<&File>, for_read: bool) -> posix::PollFd {
            let optfd = f.map(File::as_raw_fd);
            let events = if for_read { posix::POLLIN } else { posix::POLLOUT };
            posix::PollFd::new(optfd, events)
        }

        let mut fds = [to_poll(fin, false),
                       to_poll(fout, true), to_poll(ferr, true)];
        posix::poll(&mut fds, None)?;

        Ok((fds[0].test(posix::POLLOUT | posix::POLLHUP),
            fds[1].test(posix::POLLIN | posix::POLLHUP),
            fds[2].test(posix::POLLIN | posix::POLLHUP)))
    }

    fn comm_poll(stdin_ref: &mut Option<File>,
                 stdout_ref: &mut Option<File>,
                 stderr_ref: &mut Option<File>,
                 mut input_data: Option<&[u8]>)
                 -> IoResult<(Vec<u8>, Vec<u8>)> {
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
                    if let Some(input_data) = input_data {
                        stdin_ref.as_ref().unwrap().write_all(input_data)?;
                        // close stdin when done writing, so the child receives EOF
                        stdin_ref.take();
                    }
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
                _ => ()
            }

            let (in_ready, out_ready, err_ready)
                = poll3(stdin_ref.as_ref(), stdout_ref, stderr_ref)?;
            if in_ready {
                if let Some(mut input_data) = input_data {
                let chunk = &input_data[..min(WRITE_SIZE, input_data.len())];
                let n = stdin_ref.as_ref().unwrap().write(chunk)?;
                input_data = &input_data[n..];
                if input_data.is_empty() {
                    // close stdin when done writing, so the child receives EOF
                    stdin_ref.take();
                }
                } else {
                    break;
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

    pub fn communicate(stdin_ref: &mut Option<File>,
                       stdout_ref: &mut Option<File>,
                       stderr_ref: &mut Option<File>,
                       input_data: Option<&[u8]>)
                       -> IoResult<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        if !stdin_ref.is_some() {
            assert!(input_data.is_none(),
                    "cannot provide input to non-redirected stdin");
        }
        let (out, err) = comm_poll(stdin_ref, stdout_ref, stderr_ref,
                                   input_data)?;
        Ok((stdout_ref.as_ref().map(|_| out),
            stderr_ref.as_ref().map(|_| err)))
    }
}

#[cfg(windows)]
mod os {
    use std::fs::File;
    use std::io::{Read, Write, Result as IoResult};

    fn comm_read(mut outfile: File) -> IoResult<Vec<u8>> {
        // take() ensures stdin is closed when done writing, so the
        // child receives EOF
        let mut contents = Vec::new();
        outfile.read_to_end(&mut contents)?;
        Ok(contents)
    }

    fn comm_write(mut infile: File, input_data: Option<&[u8]>) -> IoResult<()> {
        if let Some(input_data) = input_data {
            infile.write_all(input_data)?;
        }
        Ok(())
    }

    pub fn comm_threaded(stdin_ref: &mut Option<File>,
                         stdout_ref: &mut Option<File>,
                         stderr_ref: &mut Option<File>,
                         input_data: Option<&[u8]>)
                         -> IoResult<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        crossbeam_utils::thread::scope(move |scope| {
            let (mut out_thr, mut err_thr) = (None, None);
            if stdout_ref.is_some() {
                out_thr = Some(scope.spawn(
                    move || comm_read(stdout_ref.take().unwrap())))
            }
            if stderr_ref.is_some() {
                err_thr = Some(scope.spawn(
                    move || comm_read(stderr_ref.take().unwrap())))
            }
            if stdin_ref.is_some() {
                comm_write(stdin_ref.take().unwrap(), input_data)?;
            }
            Ok((if let Some(out_thr) = out_thr
                { Some(out_thr.join().unwrap()?) } else { None },
                if let Some(err_thr) = err_thr
                { Some(err_thr.join().unwrap()?) } else { None }))
        })
    }

    pub fn communicate(stdin: &mut Option<File>,
                       stdout: &mut Option<File>,
                       stderr: &mut Option<File>,
                       input_data: Option<&[u8]>)
                       -> IoResult<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        match (stdin, stdout, stderr) {
            (stdin_ref @ &mut Some(..), &mut None, &mut None) => {
                comm_write(stdin_ref.take().unwrap(), input_data)?;
                Ok((None, None))
            }
            (&mut None, stdout_ref @ &mut Some(..), &mut None) => {
                assert!(input_data.is_none(),
                        "cannot provide input to non-redirected stdin");
                let out = comm_read(stdout_ref.take().unwrap())?;
                Ok((Some(out), None))
            }
            (&mut None, &mut None, stderr_ref @ &mut Some(..)) => {
                assert!(input_data.is_none(),
                        "cannot provide input to non-redirected stdin");
                let err = comm_read(stderr_ref.take().unwrap())?;
                Ok((None, Some(err)))
            }
            (ref mut stdin_ref, ref mut stdout_ref, ref mut stderr_ref) =>
                comm_threaded(stdin_ref, stdout_ref, stderr_ref, input_data)
        }
    }
}

pub use self::os::communicate;
