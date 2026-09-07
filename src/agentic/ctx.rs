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
        let audit = Audit::from_home(&home_root, &cfg);
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
        let raw = self.cfg.str_or("logging.spend_file", "logs/spend.jsonl");
        let p = PathBuf::from(&raw);
        if p.is_absolute() {
            p
        } else {
            self.home_root.join(p)
        }
    }
}
