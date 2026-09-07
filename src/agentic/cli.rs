//! `cgagentharness agentic <action>`: the out-of-band CLI the server spawns.
//!
//! Exit codes are an API (`utils/ops_runner.py`'s labels): 0 ok, 2 failed,
//! 3 env/config, 4 write refused. A panic is caught and exits 2, never 101.
//! The "layer disabled" banners are verbatim from CyClaw because the server's
//! `AGENTIC_DISABLED` translation looks for them.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::common::config::AppConfig;
use crate::common::errors::HarnessError;

pub const EXIT_OK: u8 = 0;
pub const EXIT_FAIL: u8 = 2;
pub const EXIT_ENV: u8 = 3;
pub const EXIT_REFUSED: u8 = 4;

pub const DISABLED_BANNER: &str = "Agentic layer disabled";
pub const DEEPAGENT_DISABLED_BANNER: &str = "Deep Agents / real-repo coding subsystem disabled";

/// Map a typed error to the exit-code contract.
pub fn exit_code_for(err: &HarnessError) -> u8 {
    match err.code.as_str() {
        "AGENTIC_WRITE_REFUSED" => EXIT_REFUSED,
        "GH_NOT_INSTALLED"
        | "GH_VERSION_TOO_OLD"
        | "AGENTIC_CONFIG_INVALID"
        | "CONFIG_ERROR"
        | "HARD_SANDBOX_UNAVAILABLE" => EXIT_ENV,
        _ => EXIT_FAIL,
    }
}

fn heading(title: &str) {
    println!("== {title} ==");
}

pub fn disabled_noop() -> u8 {
    heading(DISABLED_BANNER);
    println!("  agentic.enabled is false in config.yaml; nothing to do.");
    println!("  Set agentic.enabled: true to use this layer.");
    EXIT_OK
}

pub fn deepagent_disabled_noop() -> u8 {
    heading(DEEPAGENT_DISABLED_BANNER);
    println!("  agentic.deepagent_github.enabled is false in config.yaml; nothing to do.");
    println!("  Set agentic.deepagent_github.enabled: true to use this subsystem.");
    EXIT_OK
}

/// Parsed top-level invocation: `--config <path> <action> [args...]`.
pub struct Invocation {
    pub config_path: PathBuf,
    pub action: String,
    pub args: Vec<String>,
}

pub fn parse_invocation(args: &[OsString]) -> Result<Invocation, String> {
    let strs: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let mut config_path: Option<PathBuf> = None;
    let mut i = 0;
    while i < strs.len() {
        let a = &strs[i];
        if a == "--config" {
            i += 1;
            config_path = Some(PathBuf::from(strs.get(i).ok_or("--config needs a value")?));
            i += 1;
        } else if let Some(v) = a.strip_prefix("--config=") {
            config_path = Some(PathBuf::from(v));
            i += 1;
        } else {
            break;
        }
    }
    let action = strs.get(i).cloned().ok_or("a subcommand is required")?;
    Ok(Invocation {
        config_path: config_path.unwrap_or_else(|| PathBuf::from("config.yaml")),
        action,
        args: strs[i + 1..].to_vec(),
    })
}

/// Option parsing for the subcommands: `--opt=value`, `--opt value`, `--flag`,
/// repeatable `--read-file`.
#[derive(Debug, Default)]
pub struct Opts {
    pub values: std::collections::BTreeMap<String, Vec<String>>,
    pub flags: std::collections::BTreeSet<String>,
}

const VALUE_OPTS: [&str; 15] = [
    "pr",
    "issue",
    "name",
    "desc",
    "body",
    "body-file",
    "reason",
    "instruction",
    "read-file",
    "plan-file",
    "checks-file",
    "branch",
    "commit-message",
    "max-iterations",
    "run-id",
];
const VALUE_OPTS_EXTRA: [&str; 3] = ["decision", "provider", "out"];
const FLAG_OPTS: [&str; 6] = ["repo", "no-diff", "confirm", "confirm-online", "push", "publish"];
const FLAG_OPTS_EXTRA: [&str; 1] = ["confirm-publish"];

impl Opts {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut out = Opts::default();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            let Some(body) = a.strip_prefix("--") else {
                return Err(format!("unexpected argument: {a}"));
            };
            let (key, inline) = match body.split_once('=') {
                Some((k, v)) => (k.to_string(), Some(v.to_string())),
                None => (body.to_string(), None),
            };
            if FLAG_OPTS.contains(&key.as_str()) || FLAG_OPTS_EXTRA.contains(&key.as_str()) {
                if inline.is_some() {
                    return Err(format!("--{key} takes no value"));
                }
                out.flags.insert(key);
                i += 1;
                continue;
            }
            if VALUE_OPTS.contains(&key.as_str()) || VALUE_OPTS_EXTRA.contains(&key.as_str()) {
                let value = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i).cloned().ok_or_else(|| format!("--{key} needs a value"))?
                    }
                };
                out.values.entry(key).or_default().push(value);
                i += 1;
                continue;
            }
            return Err(format!("unknown option: --{key}"));
        }
        Ok(out)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).and_then(|v| v.last()).map(|s| s.as_str())
    }

    pub fn all(&self, key: &str) -> Vec<String> {
        self.values.get(key).cloned().unwrap_or_default()
    }

    pub fn flag(&self, key: &str) -> bool {
        self.flags.contains(key)
    }

    pub fn require(&self, key: &str) -> Result<String, String> {
        self.get(key)
            .map(|s| s.to_string())
            .ok_or_else(|| format!("--{key} is required"))
    }
}

/// Entry point used by `main.rs`. Never panics out: a panic becomes exit 2.
pub fn main(args: Vec<OsString>) -> u8 {
    match std::panic::catch_unwind(|| run(&args)) {
        Ok(code) => code,
        Err(_) => {
            eprintln!("agentic: internal error");
            EXIT_FAIL
        }
    }
}

fn run(args: &[OsString]) -> u8 {
    let inv = match parse_invocation(args) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("agentic: {e}");
            return EXIT_FAIL;
        }
    };
    // A hidden helper for the shim's timeout tests: never on the shim whitelist.
    if inv.action == "__sleep" {
        let secs: u64 = inv.args.first().and_then(|s| s.parse().ok()).unwrap_or(30);
        std::thread::sleep(std::time::Duration::from_secs(secs));
        return EXIT_OK;
    }
    let cfg = match AppConfig::load(&inv.config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("agentic: {}", e.message);
            return EXIT_ENV;
        }
    };
    let opts = match Opts::parse(&inv.args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("agentic {}: {e}", inv.action);
            return EXIT_FAIL;
        }
    };
    let outcome = super::commands::dispatch(&inv.action, &cfg, &inv.config_path, &opts);
    match outcome {
        Ok(code) => code,
        Err(err) => {
            eprintln!("agentic {}: {}: {}", inv.action, err.code, err.message);
            exit_code_for(&err)
        }
    }
}
