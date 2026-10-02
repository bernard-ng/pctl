//! Noninteractive process supervision. Child output belongs on stderr; stdout is
//! reserved for machine reports. Unix process groups cover ordinary descendants,
//! but cannot contain programs that deliberately create a new session/group.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

/// Shared, sticky cancellation: the first positive signal wins.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicI32>);

impl Cancellation {
    pub fn cancel(&self, signal: i32) {
        if signal > 0 {
            let _ = self
                .0
                .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst);
        }
    }

    pub fn signal(&self) -> i32 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Owns signal registrations without retaining them after the runner is dropped.
pub struct SignalGuard {
    #[cfg(unix)]
    registrations: Vec<signal_hook::SigId>,
}

impl SignalGuard {
    pub fn install(cancel: Cancellation) -> crate::Result<Self> {
        #[cfg(unix)]
        {
            let mut guard = Self {
                registrations: Vec::new(),
            };
            for signal in [libc::SIGINT, libc::SIGTERM] {
                let cancel = cancel.clone();
                // The handler only performs a lock-free atomic operation.
                let registration = unsafe {
                    signal_hook::low_level::register(signal, move || cancel.cancel(signal))
                }
                .map_err(|error| {
                    crate::error::Error::from(format!(
                        "Unable to register process signal handler: {error}"
                    ))
                })?;
                guard.registrations.push(registration);
            }
            Ok(guard)
        }
        #[cfg(not(unix))]
        {
            let _ = cancel;
            Err("Process supervision requires Unix process groups".into())
        }
    }
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        for registration in self.registrations.drain(..) {
            signal_hook::low_level::unregister(registration);
        }
    }
}

/// Run with exactly the supplied environment and no interactive stdin.
/// Timeout returns 124; cancellation returns 128 + signal. Both allow up to one
/// second before SIGKILL. Remaining group members are killed even on normal exit.
/// Pipes are drained synchronously with nonblocking reads gated by `poll(2)`, so
/// there are no output workers to join and a chatty child is never throttled by a
/// fixed sleep. After cleanup, draining is bounded to 100 ms to tolerate an
/// escaped descendant retaining a pipe. Such a descendant is outside group control.
pub fn run(
    command: &[String],
    directory: &Path,
    environment: &BTreeMap<String, String>,
    timeout: Option<Duration>,
    cancellation: &Cancellation,
    secrets: &[String],
) -> crate::Result<i32> {
    #[cfg(unix)]
    {
        unix::run(
            command,
            directory,
            environment,
            timeout,
            cancellation,
            secrets,
            &mut std::io::stderr(),
        )
    }
    #[cfg(not(unix))]
    {
        let _ = (
            command,
            directory,
            environment,
            timeout,
            cancellation,
            secrets,
        );
        Err("Process supervision requires Unix process groups".into())
    }
}

/// Collect combined stdout/stderr for a bounded version probe (at most 1 MiB).
/// Invalid UTF-8 is replaced lossily. Exceeding the limit is an error and kills
/// the process group; no partial output is returned or printed.
pub fn capture(
    command: &[String],
    directory: &Path,
    environment: &BTreeMap<String, String>,
    timeout: Option<Duration>,
    cancellation: &Cancellation,
) -> crate::Result<(i32, String)> {
    #[cfg(unix)]
    {
        let mut output = unix::Capture(Vec::new());
        let code = unix::run(
            command,
            directory,
            environment,
            timeout,
            cancellation,
            &[],
            &mut output,
        )?;
        Ok((code, String::from_utf8_lossy(&output.0).into_owned()))
    }
    #[cfg(not(unix))]
    {
        let _ = (command, directory, environment, timeout, cancellation);
        Err("Process supervision requires Unix process groups".into())
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, RawFd};
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;

    pub(super) struct Capture(pub(super) Vec<u8>);

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > 1024 * 1024 - self.0.len() {
                return Err(io::Error::other(
                    "Process capture exceeds 1 MiB output limit",
                ));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Masks the union of matching byte ranges, including overlapping secrets.
    /// Storage is bounded by the longest secret, independent of output size.
    struct Redactor {
        secrets: Vec<Vec<u8>>,
        pending: VecDeque<(u8, bool)>,
        lookahead: usize,
        masking: bool,
    }

    impl Redactor {
        fn new(secrets: &[String]) -> Self {
            let secrets: Vec<Vec<u8>> = secrets
                .iter()
                .filter(|s| !s.is_empty())
                .map(|s| s.as_bytes().to_vec())
                .collect();
            let lookahead = secrets.iter().map(Vec::len).max().unwrap_or(1);
            Self {
                secrets,
                pending: VecDeque::new(),
                lookahead,
                masking: false,
            }
        }

        fn push(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
            if self.secrets.is_empty() {
                return output.write_all(bytes);
            }
            for &byte in bytes {
                self.pending.push_back((byte, false));
                if self.pending.len() >= self.lookahead {
                    self.emit(output)?;
                }
            }
            Ok(())
        }

        fn emit(&mut self, output: &mut impl Write) -> io::Result<()> {
            for secret in &self.secrets {
                if secret.len() <= self.pending.len()
                    && secret.iter().zip(&self.pending).all(|(a, (b, _))| a == b)
                {
                    for (_, masked) in self.pending.iter_mut().take(secret.len()) {
                        *masked = true;
                    }
                }
            }
            if let Some((byte, masked)) = self.pending.pop_front() {
                if masked && !self.masking {
                    output.write_all(b"[REDACTED]")?;
                } else if !masked {
                    output.write_all(&[byte])?;
                }
                self.masking = masked;
            }
            Ok(())
        }

        fn finish(&mut self, output: &mut impl Write) -> io::Result<()> {
            while !self.pending.is_empty() {
                self.emit(output)?;
            }
            Ok(())
        }
    }

    struct Output<R> {
        pipe: R,
        redactor: Redactor,
        finished: bool,
    }

    impl<R: Read + AsRawFd> Output<R> {
        fn new(pipe: R, secrets: &[String]) -> io::Result<Self> {
            // This descriptor is owned by the child pipe and remains open here.
            let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
            if flags < 0
                || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                    < 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                pipe,
                redactor: Redactor::new(secrets),
                finished: false,
            })
        }

        /// Descriptor to wait on, or `None` once the pipe has reached EOF.
        fn pending_fd(&self) -> Option<RawFd> {
            (!self.finished).then(|| self.pipe.as_raw_fd())
        }

        fn drain(&mut self, output: &mut impl Write) -> io::Result<()> {
            if self.finished {
                return Ok(());
            }
            let mut buffer = [0; 8192];
            let mut rendered = Vec::with_capacity(8192);
            // Bound each turn so a noisy child cannot starve signal/timeout polling.
            for _ in 0..8 {
                match self.pipe.read(&mut buffer) {
                    Ok(0) => {
                        self.finished = true;
                        self.redactor.finish(&mut rendered)?;
                        break;
                    }
                    Ok(count) => self.redactor.push(&buffer[..count], &mut rendered)?,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            }
            output.write_all(&rendered)?;
            output.flush()
        }

        fn finish(&mut self, output: &mut impl Write) -> io::Result<()> {
            let mut rendered = Vec::new();
            self.redactor.finish(&mut rendered)?;
            output.write_all(&rendered)?;
            output.flush()
        }
    }

    /// Error paths also kill the group and reap the direct child.
    ///
    /// The child's pid doubles as its process-group id, so signalling `-pid` is only
    /// safe while that pid cannot have been recycled. A child that has exited but
    /// not been reaped remains a zombie and keeps the pid reserved, so the group is
    /// always signalled *before* the leader is reaped (see `has_exited`).
    struct SupervisedChild {
        process: Child,
        reaped: bool,
    }

    impl SupervisedChild {
        fn new(process: Child) -> Self {
            Self {
                process,
                reaped: false,
            }
        }

        fn signal(&self, signal: i32) {
            debug_assert!(!self.reaped, "the pid may already belong to someone else");
            // process_group(0) makes the child's pid its process group id.
            unsafe {
                libc::kill(-(self.process.id() as i32), signal);
            }
        }

        /// Whether the direct child has exited, observed without reaping it.
        fn has_exited(&self) -> io::Result<bool> {
            // SAFETY: siginfo_t is plain data for which all-zero is valid, and the
            // pointer is to a live local. si_pid stays zero if nothing is waitable.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.process.id() as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result != 0 {
                let error = io::Error::last_os_error();
                return if error.kind() == io::ErrorKind::Interrupted {
                    Ok(false)
                } else {
                    Err(error)
                };
            }
            #[cfg(any(target_os = "linux", target_os = "android"))]
            let pid = unsafe { info.si_pid() };
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            let pid = info.si_pid;
            Ok(pid != 0)
        }

        fn reap(&mut self) -> io::Result<std::process::ExitStatus> {
            let status = self.process.wait()?;
            self.reaped = true;
            Ok(status)
        }
    }

    impl Drop for SupervisedChild {
        fn drop(&mut self) {
            if !self.reaped {
                self.signal(libc::SIGKILL);
                let _ = self.process.wait();
            }
        }
    }

    /// Sleep until either pipe is readable (or hung up) or `timeout` elapses.
    /// Finished pipes are passed as -1, which `poll` ignores; with both finished
    /// this is a plain sleep. Interruption by a signal simply ends the wait early.
    fn wait_for_output(streams: [Option<RawFd>; 2], timeout: Duration) {
        let mut set = streams.map(|fd| libc::pollfd {
            fd: fd.unwrap_or(-1),
            events: libc::POLLIN,
            revents: 0,
        });
        let milliseconds = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        // `set` is a live array of two initialised pollfd structs.
        unsafe {
            libc::poll(set.as_mut_ptr(), 2, milliseconds);
        }
    }

    pub(super) fn run(
        command: &[String],
        directory: &Path,
        environment: &BTreeMap<String, String>,
        timeout: Option<Duration>,
        cancellation: &Cancellation,
        secrets: &[String],
        output: &mut impl Write,
    ) -> crate::Result<i32> {
        let program = command.first().ok_or("Cannot run an empty command")?;
        let signal = cancellation.signal();
        if signal != 0 {
            return Ok(128 + signal);
        }
        let child = Command::new(program)
            .args(&command[1..])
            .current_dir(directory)
            .env_clear()
            .envs(environment)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|error| {
                crate::error::Error::from(format!("Unable to start task process: {error}"))
            })?;
        let mut child = SupervisedChild::new(child);
        let mut stdout = Output::new(child.process.stdout.take().expect("piped stdout"), secrets)
            .map_err(|error| {
            crate::error::Error::from(format!("Unable to configure task stdout: {error}"))
        })?;
        let mut stderr = Output::new(child.process.stderr.take().expect("piped stderr"), secrets)
            .map_err(|error| {
            crate::error::Error::from(format!("Unable to configure task stderr: {error}"))
        })?;
        let started = Instant::now();
        let mut stopping: Option<(Instant, i32)> = None;
        let code = loop {
            let signal = cancellation.signal();
            if stopping.is_none() {
                let stop = if signal != 0 {
                    Some((signal, 128 + signal))
                } else if timeout.is_some_and(|limit| started.elapsed() >= limit) {
                    Some((libc::SIGTERM, 124))
                } else {
                    None
                };
                if let Some((signal, code)) = stop {
                    child.signal(signal);
                    stopping = Some((Instant::now(), code));
                }
            }
            if stopping.is_some_and(|(when, _)| when.elapsed() >= Duration::from_secs(1)) {
                child.signal(libc::SIGKILL);
            }
            if child.has_exited().map_err(|error| {
                crate::error::Error::from(format!("Unable to poll task process: {error}"))
            })? {
                // Kill remaining group members while the zombie leader still
                // reserves the group id, then reap it for the exit status.
                child.signal(libc::SIGKILL);
                let status = child.reap().map_err(|error| {
                    crate::error::Error::from(format!("Unable to reap task process: {error}"))
                })?;
                break stopping.map(|(_, code)| code).unwrap_or_else(|| {
                    status.code().unwrap_or(128 + status.signal().unwrap_or(1))
                });
            }
            stdout
                .drain(output)
                .and_then(|()| stderr.drain(output))
                .map_err(|error| {
                    crate::error::Error::from(format!("Unable to stream task output: {error}"))
                })?;
            wait_for_output(
                [stdout.pending_fd(), stderr.pending_fd()],
                Duration::from_millis(10),
            );
        };
        let draining = Instant::now();
        while !(stdout.finished && stderr.finished)
            && draining.elapsed() < Duration::from_millis(100)
        {
            stdout
                .drain(output)
                .and_then(|()| stderr.drain(output))
                .map_err(|error| {
                    crate::error::Error::from(format!("Unable to drain task output: {error}"))
                })?;
            wait_for_output(
                [stdout.pending_fd(), stderr.pending_fd()],
                Duration::from_millis(100).saturating_sub(draining.elapsed()),
            );
        }
        stdout
            .finish(output)
            .and_then(|()| stderr.finish(output))
            .map_err(|error| {
                crate::error::Error::from(format!("Unable to finish task output: {error}"))
            })?;
        Ok(code)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn redacts_every_chunk_boundary_including_multiline_and_overlaps() {
            let secrets = vec!["abc\ndef".into(), "defXYZ".into(), "".into(), "秘密".into()];
            let input = "before abc\ndefXYZ 秘密 after".as_bytes();
            for split in 0..=input.len() {
                let mut redactor = Redactor::new(&secrets);
                let mut output = Vec::new();
                redactor.push(&input[..split], &mut output).unwrap();
                redactor.push(&input[split..], &mut output).unwrap();
                redactor.finish(&mut output).unwrap();
                assert_eq!(output, b"before [REDACTED] [REDACTED] after");
            }
        }

        #[test]
        fn bounded_lookahead_preserves_partial_matches_at_eof() {
            let mut redactor = Redactor::new(&["secret".into()]);
            let mut output = Vec::new();
            for byte in b"secret!secre" {
                redactor.push(&[*byte], &mut output).unwrap();
                assert!(redactor.pending.len() < 6);
            }
            redactor.finish(&mut output).unwrap();
            assert_eq!(output, b"[REDACTED]!secre");
        }
    }
}
