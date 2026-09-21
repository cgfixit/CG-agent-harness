//! Private stdio supervisor. Linux strict mode belongs to a transient systemd
//! service, so loss of the harness or supervisor cannot orphan its cgroup.
#[cfg(target_os = "linux")]
mod linux {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::time::Duration;

    use serde::{Deserialize, Serialize};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{UnixListener, UnixStream};
    use tokio::process::{ChildStdin, Command};

    use crate::common::errors::{HarnessError, Result};
    use crate::common::mcp_policy::ResourceLimits;

    const MAX_SPEC_BYTES: usize = 65_536;

    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Spec {
        version: u32,
        argv: Vec<String>,
        cwd: PathBuf,
        env: BTreeMap<String, String>,
        owner_socket: PathBuf,
        owner_nonce: String,
        unit: String,
        timeout_sec: u64,
        limits: ResourceLimits,
    }

    pub struct Prepared {
        spec: Vec<u8>,
        nonce: String,
        listener: UnixListener,
        _directory: tempfile::TempDir,
    }

    fn unavailable() -> HarnessError {
        HarnessError::new(
            "MCP_CONTAINMENT_UNAVAILABLE",
            "strict MCP requires a working systemd service manager with cgroup v2 process and memory controllers",
        )
    }

    pub fn prepare(
        command: &mut Command,
        executable: &Path,
        timeout: Duration,
        limits: ResourceLimits,
    ) -> Result<Prepared> {
        if !Path::new("/sys/fs/cgroup/cgroup.controllers").is_file() {
            return Err(unavailable());
        }
        let systemd = super::super::process::which("systemd-run")
            .and_then(|p| dunce::canonicalize(p).ok())
            .ok_or_else(unavailable)?;
        let directory = tempfile::Builder::new().prefix("cgah-mcp-owner-").tempdir_in("/tmp")?;
        let socket = directory.path().join("owner");
        let listener = UnixListener::bind(&socket)?;
        let nonce = super::super::random_hex(32);
        let unit = format!("cgah-mcp-{}.service", super::super::random_hex(16));
        let original = command.as_std();
        let mut argv = vec![original.get_program().to_string_lossy().into_owned()];
        argv.extend(original.get_args().map(|s| s.to_string_lossy().into_owned()));
        let spec = serde_json::to_vec(&Spec {
            version: 1,
            argv,
            cwd: original.get_current_dir().ok_or_else(unavailable)?.to_path_buf(),
            env: original
                .get_envs()
                .filter_map(|(k, v)| Some((k.to_str()?.to_owned(), v?.to_str()?.to_owned())))
                .collect(),
            owner_socket: socket,
            owner_nonce: nonce.clone(),
            unit: unit.clone(),
            timeout_sec: timeout.as_secs().clamp(1, 60),
            limits,
        })?;
        if spec.len() > MAX_SPEC_BYTES {
            return Err(unavailable());
        }
        let mut managed = Command::new(systemd);
        managed.env_clear().env("PATH", "/usr/bin:/bin").env("LANG", "C");
        // The root CI fixture uses the system manager; normal operators use
        // their existing user manager. No privilege elevation or service install.
        // SAFETY: read-only effective UID lookup.
        if unsafe { libc::geteuid() } != 0 {
            managed.arg("--user");
            managed.env("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { libc::geteuid() }));
        }
        managed
            .args([
                "--quiet",
                "--pipe",
                "--wait",
                "--collect",
                "--service-type=exec",
                "--expand-environment=no",
            ])
            .arg(format!("--unit={unit}"))
            .arg(format!("--property=TasksMax={}", limits.processes))
            .arg(format!(
                "--property=MemoryMax={}",
                u64::from(limits.memory_mb) * 1024 * 1024
            ))
            .arg(format!(
                "--property=RuntimeMaxSec={}",
                timeout.as_secs().clamp(1, 60) + 2
            ))
            .args([
                "--property=KillMode=control-group",
                "--property=KillSignal=SIGKILL",
                "--property=TimeoutStopSec=1",
                "--property=SendSIGKILL=yes",
                "--property=Delegate=no",
                "--property=NoNewPrivileges=yes",
                "--property=ProtectControlGroups=yes",
                "--",
            ])
            .arg(executable)
            .arg("mcp-stdio-worker");
        managed.env_remove("DBUS_SESSION_BUS_ADDRESS");
        *command = managed;
        Ok(Prepared {
            spec,
            nonce,
            listener,
            _directory: directory,
        })
    }

    impl Prepared {
        pub async fn attach(&self, input: &mut ChildStdin) -> Result<UnixStream> {
            input.write_u32(self.spec.len() as u32).await?;
            input.write_all(&self.spec).await?;
            input.flush().await?;
            let (mut stream, _) = self.listener.accept().await?;
            // SAFETY: read-only effective UID lookup. The service uses the same identity.
            if stream.peer_cred()?.uid() != unsafe { libc::geteuid() } {
                return Err(unavailable());
            }
            let mut nonce = [0; 64];
            stream.read_exact(&mut nonce).await?;
            use subtle::ConstantTimeEq;
            if !bool::from(nonce.as_slice().ct_eq(self.nonce.as_bytes())) {
                return Err(unavailable());
            }
            stream.write_all(&[1]).await?;
            Ok(stream)
        }
    }

    fn validate_service(spec: &Spec) -> Result<()> {
        if spec.version != 1
            || !(1..=60).contains(&spec.timeout_sec)
            || !(8..=128).contains(&spec.limits.processes)
            || !(64..=4096).contains(&spec.limits.memory_mb)
            || spec.argv.is_empty()
            || !Path::new(&spec.argv[0]).is_absolute()
            || !spec.cwd.is_absolute()
            || !spec.owner_socket.is_absolute()
            || spec.owner_nonce.len() != 64
            || !spec.owner_nonce.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(unavailable());
        }
        let text = std::fs::read_to_string("/proc/self/cgroup")?;
        let path = text
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .ok_or_else(unavailable)?;
        if !path.starts_with('/')
            || Path::new(path)
                .components()
                .any(|c| c == std::path::Component::ParentDir)
            || Path::new(path).file_name().and_then(|s| s.to_str()) != Some(spec.unit.as_str())
        {
            return Err(unavailable());
        }
        let group = Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/'));
        let limit = |name: &str| -> Result<u64> {
            std::fs::read_to_string(group.join(name))?
                .trim()
                .parse::<u64>()
                .map_err(|_| unavailable())
        };
        if !group.join("cgroup.kill").exists()
            || limit("pids.max")? > u64::from(spec.limits.processes)
            || limit("memory.max")? > u64::from(spec.limits.memory_mb) * 1024 * 1024
        {
            return Err(unavailable());
        }
        Ok(())
    }

    async fn run() -> Result<()> {
        let mut input = tokio::io::stdin();
        let mut spec_bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            let n = input.read_u32().await? as usize;
            if n == 0 || n > MAX_SPEC_BYTES {
                return Err(unavailable());
            }
            spec_bytes.resize(n, 0);
            input.read_exact(&mut spec_bytes).await?;
            Ok(())
        })
        .await
        .map_err(|_| unavailable())??;
        let spec: Spec = serde_json::from_slice(&spec_bytes).map_err(|_| unavailable())?;
        validate_service(&spec)?;
        let mut owner = tokio::time::timeout(Duration::from_secs(2), async {
            let mut owner = UnixStream::connect(&spec.owner_socket).await?;
            owner.write_all(spec.owner_nonce.as_bytes()).await?;
            let mut ack = [0];
            owner.read_exact(&mut ack).await?;
            if ack != [1] {
                return Err(unavailable());
            }
            Ok(owner)
        })
        .await
        .map_err(|_| unavailable())??;
        let mut command = Command::new(&spec.argv[0]);
        command
            .args(&spec.argv[1..])
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(&spec.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| unavailable())?;
        let mut child_input = child.stdin.take().ok_or_else(unavailable)?;
        let mut message = [0];
        tokio::select! {
            _ = owner.read(&mut message) => {},
            _ = tokio::time::sleep(Duration::from_secs(spec.timeout_sec)) => {},
            _ = child.wait() => {},
            _ = tokio::io::copy(&mut input, &mut child_input) => {},
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
        // ExitType=main (systemd's default), KillMode=control-group and
        // SIGKILL terminate all remaining descendants, including setsid().
        Ok(())
    }

    pub fn main() -> u8 {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return 3;
        };
        let result = runtime.block_on(run());
        // stdin uses a blocking reader; do not let its pending read delay
        // service exit and therefore cgroup cleanup after the owner vanished.
        runtime.shutdown_timeout(Duration::from_millis(50));
        if result.is_ok() {
            0
        } else {
            eprintln!("MCP strict supervisor unavailable");
            3
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{main, prepare, Prepared};

#[cfg(not(target_os = "linux"))]
pub fn main() -> u8 {
    3
}
