//! Operator-owned stdio grants. No request or model output can change these.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::errors::{HarnessError, Result};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    Deny,
    Unrestricted,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Containment {
    Strict,
    /// Explicit exception: cleanup requires the harness to remain alive.
    ProcessGroup,
    /// Windows process ownership only; requires the explicit trusted-server exception.
    JobObject,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemPolicy {
    #[default]
    Confined,
    /// Per-server operator exception. No filesystem secrecy or integrity boundary.
    Unrestricted,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    pub processes: u32,
    pub memory_mb: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StdioCapabilities {
    pub version: u32,
    #[serde(default)]
    pub filesystem: FilesystemPolicy,
    #[serde(default)]
    pub read_roots: Vec<PathBuf>,
    #[serde(default)]
    pub write_roots: Vec<PathBuf>,
    pub network: NetworkPolicy,
    pub containment: Containment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ResourceLimits>,
}

impl StdioCapabilities {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 || self.read_roots.len() + self.write_roots.len() > 16 {
            return Err(HarnessError::config(
                "MCP capabilities require version 1 and at most 16 roots",
            ));
        }
        if self.containment == Containment::JobObject {
            if self.filesystem != FilesystemPolicy::Unrestricted
                || self.network != NetworkPolicy::Unrestricted
                || !self.read_roots.is_empty()
                || !self.write_roots.is_empty()
            {
                return Err(HarnessError::config("job_object requires explicit unrestricted filesystem and network, with no root grants; use only for trusted Windows servers"));
            }
        } else if self.filesystem != FilesystemPolicy::Confined {
            return Err(HarnessError::config(
                "unrestricted filesystem is only supported by the explicit Windows job_object exception",
            ));
        }
        match (self.containment, self.limits) {
            (Containment::Strict | Containment::JobObject, Some(limits))
                if (8..=128).contains(&limits.processes) && (64..=4096).contains(&limits.memory_mb) => {}
            (Containment::ProcessGroup, None) => {}
            _ => return Err(HarnessError::config("strict/job_object MCP containment requires limits: processes 8-128, memory_mb 64-4096; process_group cannot promise these aggregate limits")),
        }
        for root in self.read_roots.iter().chain(&self.write_roots) {
            if !root.is_absolute()
                || root.parent().is_none()
                || root.components().any(|c| c == std::path::Component::ParentDir)
            {
                return Err(HarnessError::config(
                    "MCP roots must be absolute paths below the filesystem root",
                ));
            }
        }
        Ok(())
    }

    /// Resolve again before every spawn. A removed or redirected grant must not
    /// silently become broader authority or an unconfined fallback.
    pub fn resolve_roots(&self, home: Option<&Path>) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
        self.validate()?;
        let resolve = |roots: &[PathBuf]| {
            roots
                .iter()
                .map(|root| {
                    let resolved = super::sandbox_wrap::canonical_existing(root)
                        .map_err(|_| HarnessError::new("MCP_CAPABILITY_REFUSED", "MCP grant path is unavailable"))?;
                    self.validate_resolved_root(&resolved, home)?;
                    Ok(resolved)
                })
                .collect::<Result<Vec<_>>>()
        };
        Ok((resolve(&self.read_roots)?, resolve(&self.write_roots)?))
    }

    pub fn validate_resolved_root(&self, resolved: &Path, home: Option<&Path>) -> Result<()> {
        if let Some(home) = home {
            super::sandbox_wrap::refuse_home_overlap(resolved, home)?;
        }
        if self.containment == Containment::Strict {
            for boundary in ["/proc", "/sys", "/run", "/var/run", "/dev"] {
                let boundary = Path::new(boundary);
                if resolved.starts_with(boundary) || boundary.starts_with(resolved) {
                    return Err(HarnessError::new(
                        "MCP_CAPABILITY_REFUSED",
                        "strict MCP roots cannot expose host process, device or service-control interfaces",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_schema_never_defaults_missing_authority() {
        let valid = serde_json::json!({"version":1,"network":"deny","containment":"strict","limits":{"processes":32,"memory_mb":512}});
        let policy: StdioCapabilities = serde_json::from_value(valid.clone()).unwrap();
        policy.validate().unwrap();
        for key in ["version", "network", "containment"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<StdioCapabilities>(missing).is_err());
        }
        for patch in [
            serde_json::json!({"network":"allowlist"}),
            serde_json::json!({"network":true}),
            serde_json::json!({"allow_unconfined":true}),
        ] {
            let mut invalid = valid.clone();
            invalid
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(serde_json::from_value::<StdioCapabilities>(invalid).is_err());
        }
        for root in ["/", "relative", "/tmp/../"] {
            let mut policy = policy.clone();
            policy.read_roots.push(root.into());
            assert!(policy.validate().is_err());
        }
    }

    #[test]
    fn strict_grants_cannot_expose_control_interfaces_or_ancestors() {
        let policy: StdioCapabilities = serde_json::from_value(serde_json::json!({
            "version":1,"network":"deny","containment":"strict",
            "limits":{"processes":32,"memory_mb":256}
        }))
        .unwrap();
        for root in ["/proc/self", "/sys/fs/cgroup", "/run/user/1000", "/var", "/dev"] {
            assert_eq!(
                policy.validate_resolved_root(Path::new(root), None).unwrap_err().code,
                "MCP_CAPABILITY_REFUSED"
            );
        }
        assert!(policy
            .validate_resolved_root(Path::new("/srv/tool-inputs"), None)
            .is_ok());
    }
}
#[test]
fn windows_exception_cannot_imply_confined_filesystem_or_network() {
    let valid = serde_json::json!({
        "version":1,"filesystem":"unrestricted","network":"unrestricted",
        "containment":"job_object","limits":{"processes":8,"memory_mb":256}
    });
    serde_json::from_value::<StdioCapabilities>(valid.clone())
        .unwrap()
        .validate()
        .unwrap();
    for (key, value) in [
        ("filesystem", serde_json::json!("confined")),
        ("network", serde_json::json!("deny")),
        ("read_roots", serde_json::json!(["/srv/input"])),
        ("write_roots", serde_json::json!(["/srv/output"])),
        ("containment", serde_json::json!("strict")),
        ("limits", serde_json::Value::Null),
    ] {
        let mut invalid = valid.clone();
        invalid[key] = value;
        assert!(serde_json::from_value::<StdioCapabilities>(invalid)
            .unwrap()
            .validate()
            .is_err());
    }
    let mut missing = valid;
    missing.as_object_mut().unwrap().remove("filesystem");
    assert!(serde_json::from_value::<StdioCapabilities>(missing)
        .unwrap()
        .validate()
        .is_err());
}
