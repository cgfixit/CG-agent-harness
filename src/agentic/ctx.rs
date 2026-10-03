//! Per-invocation context for the agentic child: parsed config, validated
//! agentic block, the audit sink, and the injection scanner.

use std::path::{Path, PathBuf};

use crate::common::audit::{Audit, Redactors};
use crate::common::config::AppConfig;
use crate::common::errors::Result;
use crate::common::injection::Scanner;

use super::config::{load_agentic_config, AgenticConfig};

pub struct AgenticCtx {
    pub cfg: AppConfig,
    pub acfg: AgenticConfig,
    pub config_path: PathBuf,
    /// The harness home: `config.yaml`'s parent directory.
    pub home_root: PathBuf,
    pub audit: Audit,
    pub scanner: Scanner,
}

impl AgenticCtx {
    pub fn new(cfg: AppConfig, config_path: &Path) -> Result<Self> {
        let home_root = config_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let home_root = dunce::canonicalize(&home_root).unwrap_or(home_root);
        let acfg = load_agentic_config(&cfg, &home_root)?;
        // The cloud hand-off is redacted here, before it leaves the machine,
        // so this side needs the exact values it holds as well as the shapes.
        let audit =
            Audit::from_home(&home_root, &cfg).with_literal_secrets(crate::common::audit::known_secret_values(&cfg));
        let scanner = Scanner::from_config(&cfg);
        Ok(Self {
            config_path: config_path.to_path_buf(),
            cfg,
            acfg,
            home_root,
            audit,
            scanner,
        })
    }

    pub fn redactors(&self) -> &Redactors {
        self.audit.redactors()
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.acfg.deepagent.workspace_root.join("runs")
    }

    pub fn spend_file(&self) -> PathBuf {
        crate::llm::spend::spend_path(&self.home_root, &self.cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cloud_handoff_redacts_credentials_this_process_holds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let key = "0123456789abcdef".repeat(4);
        let text = AppConfig::embedded_default().replacen(
            "  local_llm:\n",
            &format!("  local_llm:\n    api_key: '{key}'\n"),
            1,
        );
        let cfg = AppConfig::from_str(&text, &path).unwrap();
        let ctx = AgenticCtx::new(cfg, &path).unwrap();
        let sent = super::super::cloud_proposer::sanitize_handoff(
            &format!("context mentions {key} once"),
            "grok",
            &ctx.scanner,
            ctx.redactors(),
            &ctx.audit,
            10_000,
        )
        .unwrap();
        assert_eq!(sent, "context mentions [REDACTED_SECRET] once");
    }
}
