//! Run a fixed set of verification commands over a worktree inside a hard
//! sandbox. Port of `agentic/executor/runner.py`.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;

use crate::common::audit::Audit;
use crate::common::child_env::{self, Child};
use crate::common::errors::{HarnessError, Result};

use super::sandbox::{production_sandbox, HardSandbox, SandboxOutcome};

pub const DEFAULT_CHECK_TIMEOUT_SEC: u64 = 120;
const ALLOWED_ENV_VARS: [&str; 6] = [
    "PATH",
    "LANG",
    "LC_ALL",
    "PYTHONPATH",
    "VIRTUAL_ENV",
    "PYTHONIOENCODING",
];

/// Allowlisted subset + telemetry opt-outs + explicit proxy hostility.
pub fn scrubbed_env() -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = ALLOWED_ENV_VARS
        .iter()
        .filter_map(|n| std::env::var(n).ok().map(|v| (n.to_string(), v)))
        .collect();
    for (k, v) in [
        ("NO_PROXY", "*"),
        ("no_proxy", "*"),
        ("PIP_NO_INDEX", "1"),
        ("PIP_DISABLE_PIP_VERSION_CHECK", "1"),
        ("CARGO_NET_OFFLINE", "true"),
    ] {
        env.insert(k.into(), v.into());
    }
    child_env::apply(&mut env, Child::VerificationCheck);
    env
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub name: String,
    pub argv: Vec<String>,
    pub timeout_sec: u64,
}

impl Check {
    pub fn new(name: &str, argv: Vec<String>) -> Result<Self> {
        Self::with_timeout(name, argv, DEFAULT_CHECK_TIMEOUT_SEC)
    }

    pub fn with_timeout(name: &str, argv: Vec<String>, timeout_sec: u64) -> Result<Self> {
        if name.is_empty() {
            return Err(HarnessError::agentic("Check.name must be non-empty"));
        }
        if argv.is_empty() {
            return Err(HarnessError::agentic(format!("Check '{name}': argv must be non-empty")));
        }
        Ok(Self {
            name: name.to_string(),
            argv,
            timeout_sec,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: String,
    pub exit_code: i32,
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Default)]
pub struct VerificationReport {
    pub ok: bool,
    pub results: Vec<CheckResult>,
}

/// Run every check sequentially inside the sandbox. `sandbox` is for tests;
/// production callers pass `None` and get `production_sandbox(allow_linux_netns_fallback)`.
pub fn run_verification(
    worktree: &Path,
    checks: &[Check],
    audit: &Audit,
    sandbox: Option<&dyn HardSandbox>,
    prepared_root: Option<&Path>,
    allow_linux_netns_fallback: bool,
) -> Result<VerificationReport> {
    if checks.is_empty() {
        return Ok(VerificationReport {
            ok: true,
            results: Vec::new(),
        });
    }
    let owned: Option<Box<dyn HardSandbox>> = if sandbox.is_none() {
        Some(production_sandbox(allow_linux_netns_fallback)?)
    } else {
        None
    };
    let backend: &dyn HardSandbox = match sandbox {
        Some(s) => s,
        None => owned.as_deref().expect("production sandbox"),
    };
    let cargo = if checks.iter().any(|c| c.argv.first().is_some_and(|v| v == "cargo")) {
        Some(super::prepared::CargoInputs::load(worktree, prepared_root)?)
    } else {
        None
    };
    let mut env = scrubbed_env();
    let home = tempfile::Builder::new().prefix("cgah-exec-home-").tempdir()?;
    env.insert("HOME".into(), home.path().display().to_string());
    env.insert("USERPROFILE".into(), home.path().display().to_string());
    for (key, sub) in [
        ("CARGO_HOME", "cargo-home"),
        ("CARGO_TARGET_DIR", "target"),
        ("CARGO_BUILD_BUILD_DIR", "build"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ] {
        let path = home.path().join(sub);
        std::fs::create_dir_all(&path)?;
        env.insert(key.into(), path.display().to_string());
    }
    if let Some(cargo) = &cargo {
        cargo.configure(&mut env);
    }
    let reads = cargo.as_ref().map(|c| c.read_roots.as_slice()).unwrap_or(&[]);
    let mut results = Vec::new();
    for check in checks {
        let argv = if check.argv.first().is_some_and(|v| v == "cargo") {
            cargo.as_ref().expect("prepared Cargo inputs").argv(&check.argv)?
        } else {
            check.argv.clone()
        };
        let outcome: SandboxOutcome =
            backend.run_prepared(&argv, worktree, &env, check.timeout_sec, home.path(), reads);
        if outcome.exit_code == -3 {
            return Err(HarnessError::config(format!(
                "verification capture failed for '{}': {}; inspect the check before retrying",
                check.name, outcome.stderr
            )));
        }
        if outcome.timed_out && check.argv.first().is_some_and(|v| v == "cargo") {
            return Err(HarnessError::config(format!("verification check '{}' timed out after {}s; inspect resource usage or adjust its budget before retrying", check.name, check.timeout_sec)));
        }
        if outcome.exit_code != 0
            && check.argv.first().is_some_and(|v| v == "cargo")
            && cargo_preparation_failure(&outcome.stderr)
        {
            return Err(super::prepared::missing("check could not use locked sources, toolchain, or permitted output paths; review preparation and sandbox-compatible test paths"));
        }
        let ok = outcome.exit_code == 0 && !outcome.timed_out;
        audit.log(json!({
            "event": "agentic_executor_check_result", "check": check.name, "exit_code": outcome.exit_code,
            "ok": ok, "timed_out": outcome.timed_out, "sandbox": backend.name(),
        }));
        results.push(CheckResult {
            name: check.name.clone(),
            exit_code: outcome.exit_code,
            ok,
            stdout: outcome.stdout,
            stderr: outcome.stderr,
            timed_out: outcome.timed_out,
        });
    }
    Ok(VerificationReport {
        ok: results.iter().all(|r| r.ok),
        results,
    })
}

/// Cargo status verbs (right-aligned on stderr); their lines name crates and paths only.
const CARGO_STATUS_VERBS: [&str; 12] = [
    "Adding",
    "Blocking",
    "Building",
    "Checking",
    "Compiling",
    "Dirty",
    "Doc-tests",
    "Documenting",
    "Finished",
    "Fresh",
    "Locking",
    "Running",
];

/// A Cargo line that only quotes a package or target name, or relays text a
/// package's build script printed. None of them can name a preparation fault.
fn quotes_repository_text(line: &str) -> bool {
    // error: test failed, to rerun pass `-p pkg --test name` (also doctest, bench).
    if line.starts_with("error: ") && line.contains(" failed, to rerun pass `") {
        return true;
    }
    // warning: `pkg` (lib test) generated 2 warnings (run `cargo fix ...`)
    if line.starts_with("warning: `") && line.contains("` (") && line.contains(") generated ") {
        return true;
    }
    // warning: pkg@1.2.3: text / error: pkg@1.2.3: text (cargo:warning=, cargo::error=).
    line.strip_prefix("warning: ")
        .or_else(|| line.strip_prefix("error: "))
        .and_then(|rest| rest.split_once(": "))
        .and_then(|(id, _)| id.split_once('@'))
        .is_some_and(|(name, version)| {
            !name.is_empty()
                && !name.contains(char::is_whitespace)
                && version.starts_with(|c: char| c.is_ascii_digit())
                && !version.contains(char::is_whitespace)
        })
}

/// True when a failed Cargo check's stderr says preparation, not the candidate,
/// is at fault. Only Cargo's and the toolchain's own lines are matched: a rustc
/// diagnostic with a source location (header, `-->` line and snippet up to the
/// blank line), status lines, `could not compile`, the rerun hints, warning
/// counts and build-script messages quote repository code, names or text, so a
/// candidate that calls a function or names a test target `checksum` fails as
/// an ordinary check the planner can see instead of aborting the run.
fn cargo_preparation_failure(stderr: &str) -> bool {
    const PREPARATION: [&str; 7] = [
        "no matching package",
        "failed to load source",
        "failed to get",
        "checksum",
        "could not execute process",
        "operation not permitted",
        "permission denied",
        // No stale-lock phrase on purpose: shipped `protected_write_paths` keep
        // every Cargo.toml, Cargo.lock and .cargo/ out of a candidate's reach,
        // and `CargoInputs::load` already refuses a lock with no prepared
        // snapshot. A stale lock here means an operator unprotected manifests
        // and the candidate rewired local (path/workspace) dependencies: that
        // is planner feedback, not an unprepared toolchain. A new registry
        // crate still aborts above via "no matching package", because it needs
        // operator-approved `--online` preparation (see the stale-lock test).
    ];
    let lines: Vec<&str> = stderr.lines().collect();
    let mut own = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if lines
            .get(i + 1)
            .is_some_and(|next| next.trim_start().starts_with("-->"))
        {
            while i < lines.len() && !lines[i].trim().is_empty() {
                i += 1;
            }
            continue;
        }
        // `error: 2 targets failed:` lists each failed target's rerun arguments.
        if line.starts_with("error: ") && line.ends_with(" targets failed:") {
            i += 1;
            while i < lines.len() && lines[i].starts_with("    `") {
                i += 1;
            }
            continue;
        }
        let status = line
            .trim_start()
            .split_once(' ')
            .is_some_and(|(verb, _)| line.starts_with(' ') && CARGO_STATUS_VERBS.contains(&verb));
        if !status && !line.starts_with("error: could not compile `") && !quotes_repository_text(line) {
            own.push_str(&line.to_lowercase());
            own.push('\n');
        }
        i += 1;
    }
    PREPARATION.iter().any(|s| own.contains(s))
}

#[cfg(test)]
mod tests {
    use super::{cargo_preparation_failure, scrubbed_env, ALLOWED_ENV_VARS};
    use crate::common::child_env::{telemetry_opt_outs, Child};

    #[test]
    fn scrubbed_env_is_the_allowlist_plus_proxy_hostility_plus_the_verifier_opt_outs() {
        let env = scrubbed_env();
        for key in env.keys() {
            let fixed = [
                "NO_PROXY",
                "no_proxy",
                "PIP_NO_INDEX",
                "PIP_DISABLE_PIP_VERSION_CHECK",
                "CARGO_NET_OFFLINE",
            ];
            let opt_out = telemetry_opt_outs(Child::VerificationCheck)
                .iter()
                .any(|(k, _)| k == key);
            assert!(
                ALLOWED_ENV_VARS.contains(&key.as_str()) || fixed.contains(&key.as_str()) || opt_out,
                "{key} is not allowlisted, fixed or an opt-out"
            );
        }
        assert_eq!(env.get("NO_PROXY").map(String::as_str), Some("*"));
        assert_eq!(env.get("CARGO_NET_OFFLINE").map(String::as_str), Some("true"));
        for (key, value) in telemetry_opt_outs(Child::VerificationCheck) {
            assert_eq!(env.get(*key).map(String::as_str), Some(*value), "{key}");
        }
        assert!(
            !env.contains_key("GH_NO_UPDATE_NOTIFIER"),
            "the verifier set is not the gh set"
        );
        assert!(!env.contains_key("HTTPS_PROXY"));
    }

    #[test]
    fn candidate_code_naming_a_preparation_word_is_an_ordinary_check_failure() {
        // Real cargo 1.89 stderr for a crate named `checksum-tool` calling `checksum()`.
        let compile_error = "    Checking checksum-tool v0.1.0 (/w/checksum-tool)\n\
error[E0425]: cannot find function `checksum` in this scope\n \
--> src/lib.rs:1:14\n  |\n1 | pub fn f() { checksum() }\n  |              ^^^^^^^^ not found in this scope\n\n\
For more information about this error, try `rustc --explain E0425`.\n\
error: could not compile `checksum-tool` (lib) due to 1 previous error\n";
        assert!(!cargo_preparation_failure(compile_error));
        let macro_error = "error: cannot find macro `permission_denied` in this scope\n --> src/lib.rs:2:5\n  |\n2 |     permission_denied!();\n  |     ^^^^^^^^^^^^^^^^^\n\n";
        assert!(!cargo_preparation_failure(macro_error));
    }

    #[test]
    fn cargo_lines_quoting_names_or_build_script_text_are_ordinary_check_failures() {
        // Real cargo 1.89 and 1.97 stderr (paths shortened to /w). Each is a
        // failed test or build script the planner can fix, not a missing
        // preparation.
        for stderr in [
            // A test target named `checksum`.
            "     Running tests/checksum.rs (/w/target/debug/deps/checksum-8a64cdff760b36d2)\n\
error: test failed, to rerun pass `--test checksum`\n",
            // A workspace package named `checksum-tool`, then --no-fail-fast's list.
            "error: test failed, to rerun pass `-p checksum-tool --lib`\n   \
Doc-tests other\nerror: doctest failed, to rerun pass `-p other --doc`\n\
error: 2 targets failed:\n    `-p checksum-tool --lib`\n    `-p other --doc`\n",
            // Its compiler-warning summary (1.97 adds the `cargo fix` hint).
            "warning: `checksum-tool` (lib) generated 1 warning (run `cargo fix --lib -p checksum-tool` to apply 1 suggestion)\n\
warning: `checksum-tool` (lib test) generated 1 warning (1 duplicate)\n\
error: test failed, to rerun pass `--lib`\n",
            // Text a build script printed through cargo:warning= and cargo::error=.
            "warning: demo2@0.1.0: failed to get git revision; using unknown\n\
error: test failed, to rerun pass `--lib`\n",
            "   Compiling demo5 v0.1.0 (/w/e)\n\
error: demo5@0.1.0: checksum mismatch in generated table\nerror: build script logged errors\n",
        ] {
            assert!(!cargo_preparation_failure(stderr), "{stderr}");
        }
        // Cargo's own preparation error on the next line still aborts.
        for stderr in [
            "warning: demo2@0.1.0: using unknown revision\nerror: no matching package named `serde` found\n",
            "error: 2 targets failed:\n    `--lib`\nerror: could not execute process `rustc -vV` (never executed)\n",
        ] {
            assert!(cargo_preparation_failure(stderr), "{stderr}");
        }
    }

    #[test]
    fn cargo_and_toolchain_preparation_errors_still_abort() {
        for stderr in [
            "error: failed to get `serde` as a dependency of package `x v0.1.0 (/w)`\n\nCaused by:\n  failed to load source for dependency `serde`\n",
            "error: no matching package named `serde` found\n",
            "error: checksum for `itoa v1.0.0` changed between lock files\n",
            "error: could not execute process `rustc -vV` (never executed)\n\nCaused by:\n  Permission denied (os error 13)\n",
            "error: linking with `cc` failed: exit status: 1\n  |\n  = note: ld: cannot open output file: Operation not permitted\n",
            "    Compiling x v0.1.0 (/w)\nerror: couldn't create a temp dir: Permission denied (os error 13) at path \"/w/target\"\n",
        ] {
            assert!(cargo_preparation_failure(stderr), "{stderr}");
        }
    }

    #[test]
    fn a_candidate_made_stale_lock_is_an_ordinary_check_failure() {
        // Real stderr when a manifest edit outdates the lock under the prepared
        // `--frozen` argv: cargo 1.89 wording, then cargo 1.97 wording. Both put
        // the path mid-sentence, so no fixed phrase spans them.
        for flag in ["--frozen", "--locked"] {
            for stderr in [
                format!("error: cannot update the lock file /w/Cargo.lock because {flag} was passed to prevent this\n"),
                format!("error: the lock file /w/Cargo.lock needs to be updated but {flag} was passed to prevent this\nIf you want to try to generate the lock file without accessing the network, remove the {flag} flag and use --offline instead.\n"),
            ] {
                assert!(!cargo_preparation_failure(&stderr), "{stderr}");
            }
        }
    }
}
