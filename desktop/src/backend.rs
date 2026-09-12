use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

pub const PROTOCOL: u64 = 2;

#[path = "../../src/common/local_tls.rs"]
mod local_tls;

pub fn home() -> Result<PathBuf, String> {
    if let Some(value) = std::env::var_os("CGAGENTHARNESS_HOME").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(value);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err("Desktop home must be an absolute path.".into())
        };
    }
    for name in ["USERPROFILE", "HOME"] {
        if let Some(value) = std::env::var_os(name).filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(value).join(".CGagentHarness"));
        }
    }
    Err("No application home is configured.".into())
}

/// Bounded known directories only; never trust launch cwd or source shell files.
pub fn finder_path() -> String {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let cargo = PathBuf::from(home).join(".cargo/bin");
        if cargo.is_dir() {
            dirs.push(cargo.to_string_lossy().to_string());
        }
    }
    dirs.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ]
        .map(str::to_owned),
    );
    dirs.join(":")
}

pub fn executable(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    finder_path().split(':').map(|d| Path::new(d).join(name)).find(|p| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// Explicit operator tool directories, outside arbitrary working-directory PATH.
pub fn configured_path(home: &Path) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    let config = home.join("desktop-tools.json");
    let metadata = match std::fs::symlink_metadata(&config) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(finder_path()),
        Err(_) => return Err("Cannot read desktop-tools.json.".into()),
    };
    // SAFETY: getuid has no arguments.
    let uid = unsafe { libc::getuid() };
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 || metadata.len() > 8192 {
        return Err("desktop-tools.json must be a private regular file owned by this user, at most 8 KiB.".into());
    }
    let value: Value =
        serde_json::from_slice(&std::fs::read(config).map_err(|_| "Cannot read desktop tool configuration.")?)
            .map_err(|_| "desktop-tools.json is invalid JSON.")?;
    let directories = value["directories"]
        .as_array()
        .filter(|v| v.len() <= 8)
        .ok_or("desktop-tools.json requires at most eight explicit directories.")?;
    let mut path = Vec::new();
    for directory in directories {
        let name = directory.as_str().ok_or("Invalid explicit tool directory.")?;
        let directory = Path::new(name);
        if !directory.is_absolute() || name.contains(':') {
            return Err("Tool directories must be absolute paths without colons.".into());
        }
        let metadata = directory
            .metadata()
            .map_err(|_| "An explicitly configured tool directory is unavailable.")?;
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
            return Err("Explicit tool directories must be owned by this user and not writable by other users.".into());
        }
        path.push(name.to_string());
    }
    path.push(finder_path());
    Ok(path.join(":"))
}

pub struct Backend {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
    buffer: Vec<u8>,
    pub origin: String,
    pub hello: Value,
    pub certificate_der: Vec<u8>,
}

impl Backend {
    pub fn start(home: &Path, initialize_key: Option<String>) -> Result<Self, String> {
        let bundle_exe = std::env::current_exe().map_err(|_| "Cannot locate desktop executable.")?;
        let path = bundle_exe
            .parent()
            .ok_or("Invalid bundle layout.")?
            .join("cgagentharness");
        let bytes = std::fs::read(&path).map_err(|_| "Bundled backend missing. Reinstall the complete app.")?;
        if hex::encode(Sha256::digest(bytes)) != env!("CGAH_BACKEND_SHA256") {
            return Err("Bundled backend integrity mismatch. Reinstall the complete app.".into());
        }
        let mut child = Command::new(&path)
            .arg("desktop")
            .current_dir("/")
            .env("CGAGENTHARNESS_HOME", home)
            .env("PATH", configured_path(home)?)
            .env("RUSTUP_AUTO_INSTALL", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Cannot launch bundled backend.")?;
        let input = child.stdin.take();
        let output = child.stdout.take().ok_or("Missing private readiness channel.")?;
        // SAFETY: output is an owned live pipe, only this owner reads it.
        let flags = unsafe { libc::fcntl(output.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(output.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Cannot configure private readiness channel.".into());
        }
        let mut owner = Self {
            child,
            input,
            output,
            buffer: Vec::new(),
            origin: String::new(),
            hello: Value::Null,
            certificate_der: Vec::new(),
        };
        let mut random = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut random);
        let challenge = hex::encode(random);
        owner.send(json!({"protocol":PROTOCOL,"challenge":challenge,"initialize_key":initialize_key}))?;
        let hello = owner.read(Duration::from_secs(20))?;
        if hello.get("error").is_some() {
            return Err(hello["message"].as_str().unwrap_or("Backend setup failed.").to_string());
        }
        if hello["protocol"].as_u64() != Some(PROTOCOL)
            || hello["challenge"].as_str() != Some(&challenge)
            || hello["pid"].as_u64() != Some(owner.child.id() as u64)
        {
            return Err("Backend identity or protocol mismatch.".into());
        }
        let port = hello["port"]
            .as_u64()
            .filter(|p| *p > 0 && *p <= 65535)
            .ok_or("Invalid owned listener.")?;
        let scheme = hello["scheme"]
            .as_str()
            .filter(|s| matches!(*s, "http" | "https"))
            .ok_or("Invalid backend transport.")?;
        let origin = format!("{scheme}://127.0.0.1:{port}");
        let mut client = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .tls_info(true);
        if scheme == "https" {
            use base64::Engine;
            let encoded = hello["certificate_der"]
                .as_str()
                .filter(|s| s.len() <= 8192)
                .ok_or("Missing private-channel certificate identity.")?;
            owner.certificate_der = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| "Invalid private-channel certificate.")?;
            if hello["certificate_sha256"].as_str()
                != Some(hex::encode(Sha256::digest(&owner.certificate_der)).as_str())
            {
                return Err("Backend certificate identity mismatch.".into());
            }
            client =
                client.use_preconfigured_tls(local_tls::client_config(owner.certificate_der.clone(), "127.0.0.1")?);
        }
        let client = client.build().map_err(|_| "Cannot create readiness verifier.")?;
        let response = client
            .get(format!("{origin}/_desktop/ready"))
            .header("x-cgah-readiness", challenge)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|_| "Owned backend readiness authentication failed.")?;
        if scheme == "https"
            && response
                .extensions()
                .get::<reqwest::tls::TlsInfo>()
                .and_then(|t| t.peer_certificate())
                != Some(owner.certificate_der.as_slice())
        {
            return Err("Owned listener presented a different certificate.".into());
        }
        let response: Value = response
            .json()
            .map_err(|_| "Malformed owned listener readiness response.")?;
        if response["protocol"].as_u64() != Some(PROTOCOL) || response["pid"].as_u64() != Some(owner.child.id() as u64)
        {
            return Err("Owned listener identity mismatch.".into());
        }
        owner.origin = origin;
        owner.hello = hello;
        Ok(owner)
    }

    fn send(&mut self, frame: Value) -> Result<(), String> {
        let input = self.input.as_mut().ok_or("Backend is stopping.")?;
        let mut bytes = serde_json::to_vec(&frame).map_err(|_| "Invalid control frame.")?;
        bytes.push(b'\n');
        if bytes.len() > 8192 {
            return Err("Control request exceeds its bound.".into());
        }
        input
            .write_all(&bytes)
            .map_err(|_| "Backend control channel closed.".into())
    }

    fn read(&mut self, timeout: Duration) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = self.buffer.drain(..=end).collect();
                return serde_json::from_slice(&line).map_err(|_| "Invalid backend control response.".into());
            }
            if self.buffer.len() > 16384 {
                return Err("Backend control response exceeded its bound.".into());
            }
            if Instant::now() >= deadline {
                return Err("Backend response timed out. Retry from setup.".into());
            }
            let mut buf = [0u8; 2048];
            match self.output.read(&mut buf) {
                Ok(0) => return Err("Backend stopped unexpectedly. Inspect interrupted work before retrying.".into()),
                Ok(n) => self.buffer.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => return Err("Backend control channel failed.".into()),
            }
        }
    }

    pub fn status(&mut self) -> Result<Value, String> {
        self.send(json!({"command":"status"}))?;
        self.read(Duration::from_secs(2))
    }

    pub fn models(&mut self) -> Result<Value, String> {
        self.send(json!({"command":"models"}))?;
        self.read(Duration::from_secs(6))
    }

    pub fn stop(&mut self) {
        if self.input.is_none() {
            return;
        }
        let _ = self.send(json!({"command":"shutdown"}));
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return;
                }
            }
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn allowed_navigation(url: &tauri::Url, origin: &str) -> bool {
    url.origin().ascii_serialization() == origin && url.username().is_empty() && url.password().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_rejects_other_ports_remote_content_and_custom_schemes() {
        let origin = "http://127.0.0.1:54321";
        for address in ["http://127.0.0.1:54321/", "http://127.0.0.1:54321/#agent-job=x"] {
            assert!(allowed_navigation(&address.parse().unwrap(), origin));
        }
        for address in [
            "https://127.0.0.1:54321/",
            "http://127.0.0.1:54322/",
            "http://localhost:54321/",
            "https://example.com",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "http://secret@127.0.0.1:54321/",
        ] {
            assert!(!allowed_navigation(&address.parse().unwrap(), origin));
        }
    }
}
