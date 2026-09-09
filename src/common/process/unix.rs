//! Nonblocking pipes under one deadline. Darwin ancestry cleanup is best-effort.
use super::{build_command, Output, ProcessError, RunSpec, MAX_CAPTURE_BYTES};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn nonblocking(fd: &impl AsRawFd) -> Result<(), ProcessError> {
    // SAFETY: these are owned live pipe descriptors. Preserve their existing flags.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(ProcessError::Capture(std::io::Error::last_os_error().to_string()));
    }
    Ok(())
}

fn read_chunk<R: Read>(pipe: &mut Option<R>, bytes: &mut Vec<u8>, total: &mut usize) -> Result<(), ProcessError> {
    let Some(reader) = pipe.as_mut() else { return Ok(()) };
    let mut buffer = [0u8; 8192];
    match reader.read(&mut buffer) {
        Ok(0) => *pipe = None,
        Ok(n) => {
            if *total + n > MAX_CAPTURE_BYTES {
                return Err(ProcessError::Capture(format!(
                    "output exceeded {MAX_CAPTURE_BYTES} bytes; incomplete output refused"
                )));
            }
            *total += n;
            bytes.extend_from_slice(&buffer[..n]);
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ) => {}
        Err(e) => return Err(ProcessError::Capture(format!("pipe read failed: {e}"))),
    }
    Ok(())
}

struct Owner {
    child: Child,
    reaped: bool,
    cleaned: bool,
    #[cfg(target_os = "macos")]
    identities: Vec<Identity>,
}

impl Owner {
    fn new(child: Child) -> Self {
        Self {
            #[cfg(target_os = "macos")]
            identities: identity(child.id()).into_iter().collect(),
            child,
            reaped: false,
            cleaned: false,
        }
    }

    #[cfg(target_os = "macos")]
    fn observe(&mut self, stop: bool) {
        let started = Instant::now();
        let mut index = 0;
        // Fixed safety ceilings: cleanup must not block the Tokio worker indefinitely.
        while index < self.identities.len()
            && self.identities.len() < 1024
            && started.elapsed() < Duration::from_millis(100)
        {
            let parent = self.identities[index];
            index += 1;
            if !parent.matches() {
                continue;
            }
            if stop {
                parent.signal(libc::SIGSTOP);
            }
            let mut pids = [0i32; 256];
            // SAFETY: writable PID buffer; this API returns a COUNT, not byte length.
            let count = unsafe {
                libc::proc_listchildpids(
                    parent.pid as i32,
                    pids.as_mut_ptr().cast(),
                    std::mem::size_of_val(&pids) as i32,
                )
            };
            for pid in pids.iter().take((count.max(0) as usize).min(pids.len())) {
                if let Some(child) = identity(*pid as u32) {
                    if child.parent == parent.pid
                        && !self
                            .identities
                            .iter()
                            .any(|p| p.pid == child.pid && p.start == child.start)
                    {
                        self.identities.push(child);
                    }
                }
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn observe(&mut self, _stop: bool) {}
}

impl Owner {
    fn cleanup(&mut self) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;
        #[cfg(target_os = "macos")]
        {
            self.observe(true);
            // Descendants are admitted only through observed parentage. Recheck identity
            // before each signal. This narrows, but does not eliminate, PID reuse races.
            for process in self.identities.iter().rev() {
                process.signal(libc::SIGKILL);
            }
        }
        if !self.reaped {
            // The direct child is still owned/unreaped, so its PID cannot be reused.
            // SAFETY: its process group was established by build_command.
            unsafe {
                libc::killpg(self.child.id() as i32, libc::SIGKILL);
            }
            let _ = self.child.kill();
            // Reaping is deferred to the caller on success, preserving exit status.
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.cleanup();
        if !self.reaped {
            let _ = self.child.wait();
        }
    }
}

/// Observe exit without reaping: retain PID ownership until group cleanup.
fn exited(child: &Child) -> Result<bool, ProcessError> {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: this is our unreaped child, and a correctly sized siginfo buffer.
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if rc < 0 {
        return Err(ProcessError::Capture(format!(
            "child wait failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: waitid populated siginfo (zero PID means no exit event).
    Ok(unsafe { info.assume_init().si_pid() } == child.id() as i32)
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct Identity {
    pid: u32,
    parent: u32,
    start: (u64, u64),
}
#[cfg(target_os = "macos")]
fn identity(pid: u32) -> Option<Identity> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    // SAFETY: correctly sized output for the public PROC_PIDTBSDINFO flavor.
    let count = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            std::mem::size_of::<libc::proc_bsdinfo>() as i32,
        )
    };
    if count != std::mem::size_of::<libc::proc_bsdinfo>() as i32 {
        return None;
    }
    // SAFETY: the API filled the entire structure.
    let info = unsafe { info.assume_init() };
    Some(Identity {
        pid,
        parent: info.pbi_ppid,
        start: (info.pbi_start_tvsec, info.pbi_start_tvusec),
    })
}
#[cfg(target_os = "macos")]
impl Identity {
    fn matches(self) -> bool {
        identity(self.pid).is_some_and(|p| p.start == self.start)
    }
    fn signal(self, signal: i32) {
        if self.matches() {
            // SAFETY: observed descendant with matching process start identity.
            // The check/signal pair is not atomic and is not a containment claim.
            unsafe {
                libc::kill(self.pid as i32, signal);
            }
        }
    }
}

pub(super) fn run(spec: RunSpec<'_>, cancelled: &AtomicBool) -> Result<Output, ProcessError> {
    let started = Instant::now();
    if cancelled.load(Ordering::Acquire) {
        return Err(ProcessError::Capture("command cancelled".into()));
    }
    let mut owner = Owner::new(build_command(&spec).spawn().map_err(ProcessError::Spawn)?);
    let mut input = owner.child.stdin.take();
    let mut stdout = owner.child.stdout.take();
    let mut stderr = owner.child.stderr.take();
    if let Some(pipe) = &input {
        nonblocking(pipe)?;
    }
    if let Some(pipe) = &stdout {
        nonblocking(pipe)?;
    }
    if let Some(pipe) = &stderr {
        nonblocking(pipe)?;
    }
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut total = 0;
    let mut written = 0;
    let data = spec.stdin.unwrap_or_default();
    let mut observed = Instant::now() - Duration::from_secs(1);
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(ProcessError::Capture("command cancelled".into()));
        }
        if started.elapsed() >= spec.timeout {
            return Err(ProcessError::Timeout {
                timeout_sec: spec.timeout.as_secs_f64(),
            });
        }
        if observed.elapsed() >= Duration::from_millis(50) {
            owner.observe(false);
            observed = Instant::now();
        }
        if let Some(pipe) = input.as_mut() {
            if written == data.len() {
                input = None;
            } else {
                match pipe.write(&data[written..data.len().min(written + 8192)]) {
                    Ok(n) => written += n,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => input = None,
                    Err(e) => return Err(ProcessError::Capture(format!("stdin write failed: {e}"))),
                }
            }
        }
        read_chunk(&mut stdout, &mut out, &mut total)?;
        read_chunk(&mut stderr, &mut err, &mut total)?;
        if stdout.is_none() && stderr.is_none() && exited(&owner.child)? {
            owner.cleanup();
            let exit = owner
                .child
                .wait()
                .map_err(|e| ProcessError::Capture(format!("child reap failed: {e}")))?;
            owner.reaped = true;
            return Ok(Output {
                status: exit.code(),
                stdout: String::from_utf8_lossy(&out).into_owned(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
                timed_out: false,
            });
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
