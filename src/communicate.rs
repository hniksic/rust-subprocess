use std::fs::File;
use std::io::{Result as IoResult, Read, Write};

#[cfg(unix)]
mod os {
    use posix;
    use std::fs::File;
    use std::io::{Read, Write, Result as IoResult};
    use std::os::unix::io::AsRawFd;
    use std::cmp::min;

    fn poll3(fin: Option<&File>, fout: Option<&File>, ferr: Option<&File>)
             -> IoResult<(bool, bool, bool)> {
        fn to_poll(f: Option<&File>, for_read: bool) -> posix::PollFd {
            posix::PollFd {
                fd: f.map(File::as_raw_fd).unwrap_or(-1),
                events: if for_read { posix::POLLIN } else { posix::POLLOUT },
                revents: 0,
            }
        }

        let mut fds = [to_poll(fin, false),
                       to_poll(fout, true), to_poll(ferr, true)];
        posix::poll(&mut fds, -1)?;

        Ok((fds[0].revents & (posix::POLLOUT | posix::POLLHUP) != 0,
            fds[1].revents & (posix::POLLIN | posix::POLLHUP) != 0,
            fds[2].revents & (posix::POLLIN | posix::POLLHUP) != 0))
    }

    pub fn rw3way(stdin_ref: &mut Option<File>, stdout_ref: &mut Option<File>,
                  stderr_ref: &mut Option<File>, input_data: Option<&[u8]>)
                  -> IoResult<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        const WRITE_SIZE: usize = 4096;

        let mut stdout_ref = stdout_ref.as_ref();
        let mut stderr_ref = stderr_ref.as_ref();

        let mut input_data = input_data.unwrap_or(b"");
        let mut out = Vec::<u8>::new();
        let mut err = Vec::<u8>::new();

        while stdin_ref.is_some() || stdout_ref.is_some() || stderr_ref.is_some() {
            let (in_ready, out_ready, err_ready)
                = poll3(stdin_ref.as_ref(), stdout_ref, stderr_ref)?;
            if in_ready {
                let chunk = &input_data[..min(WRITE_SIZE, input_data.len())];
                let n = stdin_ref.as_ref().unwrap().write(chunk)?;
                input_data = &input_data[n..];
                if input_data.is_empty() {
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
        Ok((Some(out), Some(err)))
    }
}

fn comm_read(outfile: &mut Option<File>) -> IoResult<Vec<u8>> {
    let mut outfile = outfile.take().expect("file missing");
    let mut contents = Vec::new();
    outfile.read_to_end(&mut contents)?;
    Ok(contents)
}

fn comm_write(infile: &mut Option<File>, input_data: &[u8]) -> IoResult<()> {
    let mut infile = infile.take().expect("file missing");
    infile.write_all(input_data)?;
    Ok(())
}

pub fn communicate_bytes(stdin: &mut Option<File>,
                         stdout: &mut Option<File>,
                         stderr: &mut Option<File>,
                         input_data: Option<&[u8]>)
                         -> IoResult<(Option<Vec<u8>>, Option<Vec<u8>>)> {
    match (stdin, stdout, stderr) {
        (mut stdin_ref @ &mut Some(_), &mut None, &mut None) => {
            let input_data = input_data.expect(
                "must provide input to redirected stdin");
            comm_write(stdin_ref, input_data)?;
            Ok((None, None))
        }
        (&mut None, mut stdout_ref @ &mut Some(_), &mut None) => {
            assert!(input_data.is_none(),
                    "cannot provide input to non-redirected stdin");
            let out = comm_read(stdout_ref)?;
            Ok((Some(out), None))
        }
        (&mut None, &mut None, mut stderr_ref @ &mut Some(_)) => {
            assert!(input_data.is_none(),
                    "cannot provide input to non-redirected stdin");
            let err = comm_read(stderr_ref)?;
            Ok((None, Some(err)))
        }
        (ref mut stdin_ref, ref mut stdout_ref, ref mut stderr_ref) =>
            os::rw3way(stdin_ref, stdout_ref, stderr_ref, input_data)
    }
}
