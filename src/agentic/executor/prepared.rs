//! Operator-prepared, lock-bound Cargo sources. No dependency retrieval here.
use crate::common::errors::{HarnessError, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    lock_sha256: String,
    toolchain_root: PathBuf,
    runtime_roots: Vec<PathBuf>,
    rustc_version: String,
    #[serde(default)]
    sdk_root: Option<PathBuf>,
    #[serde(default)]
    linker: Option<PathBuf>,
}

pub struct CargoInputs {
    pub cargo: PathBuf,
    pub config: PathBuf,
    pub read_roots: Vec<PathBuf>,
    pub toolchain: PathBuf,
    sdk_root: Option<PathBuf>,
    linker: Option<PathBuf>,
}

pub(crate) fn missing(detail: &str) -> HarnessError {
    HarnessError::config(format!("offline Cargo verification is not prepared: {detail}; run scripts/prepare-cargo.py REPOSITORY HARNESS_HOME after provisioning the required toolchain/dependencies (use --online only for authorized preparation)"))
}

fn executable(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

impl CargoInputs {
    pub fn load(worktree: &Path, prepared_root: Option<&Path>) -> Result<Self> {
        let lock = std::fs::read(worktree.join("Cargo.lock")).map_err(|_| missing("Cargo.lock is required"))?;
        let digest = crate::common::sha256_bytes_hex(&lock);
        let root = prepared_root.ok_or_else(|| missing("prepared source root is absent"))?;
        let snapshot =
            dunce::canonicalize(root.join(&digest)).map_err(|_| missing("no snapshot for this Cargo.lock"))?;
        let candidate = dunce::canonicalize(worktree)?;
        if snapshot.starts_with(&candidate) || candidate.starts_with(&snapshot) {
            return Err(missing("prepared sources must be separate from the candidate"));
        }
        let manifest: Manifest = serde_json::from_slice(
            &std::fs::read(snapshot.join("prepared.json")).map_err(|_| missing("prepared.json is missing"))?,
        )
        .map_err(|_| missing("invalid prepared.json"))?;
        if manifest.lock_sha256 != digest || std::fs::read(snapshot.join("Cargo.lock"))? != lock {
            return Err(missing("snapshot lock mismatch"));
        }
        let toolchain = dunce::canonicalize(&manifest.toolchain_root).map_err(|_| missing("toolchain disappeared"))?;
        if toolchain.parent().is_none() || manifest.rustc_version.is_empty() {
            return Err(missing("invalid toolchain root"));
        }
        for tool in ["cargo", "rustc", "rustdoc"] {
            if !toolchain.join("bin").join(executable(tool)).is_file() {
                return Err(missing(&format!("missing {tool}")));
            }
        }
        let config = snapshot.join("config.toml");
        if !config.is_file() {
            return Err(missing("source configuration is missing"));
        }
        let mut read_roots = vec![snapshot, toolchain.clone()];
        for path in manifest.runtime_roots {
            let path = dunce::canonicalize(path).map_err(|_| missing("compiler runtime disappeared"))?;
            if !path.starts_with("/opt/homebrew/Cellar") && !path.starts_with("/usr/local/Cellar") {
                return Err(missing("unsupported compiler runtime location"));
            }
            read_roots.push(path);
        }
        Ok(Self {
            cargo: toolchain.join("bin").join(executable("cargo")),
            config,
            read_roots,
            toolchain,
            sdk_root: manifest.sdk_root,
            linker: manifest.linker,
        })
    }

    pub fn configure(&self, env: &mut BTreeMap<String, String>) {
        env.insert(
            "RUSTC".into(),
            self.toolchain
                .join("bin")
                .join(executable("rustc"))
                .display()
                .to_string(),
        );
        env.insert(
            "RUSTDOC".into(),
            self.toolchain
                .join("bin")
                .join(executable("rustdoc"))
                .display()
                .to_string(),
        );
        env.insert("RUSTUP_AUTO_INSTALL".into(), "0".into());
        if let Some(sdk) = &self.sdk_root {
            env.insert("SDKROOT".into(), sdk.display().to_string());
        }
        if let Some(linker) = &self.linker {
            env.insert(
                "CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER".into(),
                linker.display().to_string(),
            );
            env.insert(
                "CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER".into(),
                linker.display().to_string(),
            );
        }
        let mut paths = vec![self.toolchain.join("bin")];
        if cfg!(windows) {
            paths.extend(std::env::split_paths(env.get("PATH").map(String::as_str).unwrap_or("")));
        } else {
            paths.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from));
        }
        env.insert(
            "PATH".into(),
            std::env::join_paths(paths)
                .expect("valid tool paths")
                .to_string_lossy()
                .into(),
        );
    }

    pub fn argv(&self, original: &[String]) -> Result<Vec<String>> {
        let sub = original.get(1).map(String::as_str).unwrap_or("");
        if !["test", "check", "build", "clippy", "fmt"].contains(&sub) {
            return Err(missing("unsupported Cargo check command"));
        }
        if !self
            .toolchain
            .join("bin")
            .join(executable(&format!("cargo-{sub}")))
            .is_file()
            && ["clippy", "fmt"].contains(&sub)
        {
            return Err(missing(&format!("missing {sub} component")));
        }
        let mut out = vec![
            self.cargo.display().to_string(),
            "--config".into(),
            self.config.display().to_string(),
        ];
        let split = original.iter().position(|a| a == "--").unwrap_or(original.len());
        out.extend_from_slice(&original[1..split]);
        if sub != "fmt" {
            out.push("--frozen".into());
        }
        out.extend_from_slice(&original[split..]);
        Ok(out)
    }
}
