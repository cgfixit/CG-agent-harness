//! Jailed clone: reads, path-validated writes, and four fixed git subcommands.
//! Port of `agentic/deepagent_github/repo_workspace.py`.
//!
//! Reads: a capability `Dir` held open on the clone (`openat`-style component-wise
//! resolution, so a symlink can never escape and there is no canonicalize-then-open
//! window) plus `O_NOFOLLOW` on the leaf (unix). Writes:
//! canonical path -> per-segment `.git` name-equivalence refusal -> resolve
//! (strict for `add`; dangling-leaf-aware for `write_file`) -> containment ->
//! landed-path vs the real `.git` dir -> return the LANDED relative path.
//! Every mutation reloads the write policy and requires explicit human intent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use unicode_normalization::UnicodeNormalization;

use crate::common::audit::Audit;
use crate::common::errors::{HarnessError, Result};
use crate::common::identity;
use crate::common::process::{self, RunSpec};

use super::ctx::AgenticCtx;
use super::gh_client::{run_read, ReadRequest, DEFAULT_CLONE_TIMEOUT_SEC};

pub const DEFAULT_MAX_READ_BYTES: u64 = 256_000;
pub const DEFAULT_MAX_WRITE_BYTES: usize = 256_000;
pub const DEFAULT_GIT_WRITE_TIMEOUT_SEC: u64 = 30;
pub const DEFAULT_PUSH_TIMEOUT_SEC: u64 = 120;
const GIT_ENV_ALLOWLIST: [&str; 4] = ["PATH", "HOME", "LANG", "LC_ALL"];

/// Canonical repo-relative form, or `None` if unsafe (the WRITE-path rule).
pub fn canonical_repo_path(target: &str) -> Option<String> {
    if target.is_empty() || target.contains('\0') {
        return None;
    }
    let normalized = target.replace('\\', "/");
    if normalized.starts_with('/')
        || normalized.starts_with('-')
        || crate::common::repo_paths::is_windows_absolute(target)
    {
        return None;
    }
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.is_empty() || parts.iter().any(|p| *p == ".." || p.contains(':')) {
        return None;
    }
    Some(parts.join("/"))
}

fn is_format_control(c: char) -> bool {
    matches!(c as u32, 0x00AD | 0x061C | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xE0001 | 0xE0020..=0xE007F)
}

/// True when a filesystem could resolve `part` to the `.git` directory
/// (trailing dots/spaces, NTFS `git~1`, HFS-ignored codepoints, case folding).
pub fn is_dotgit_name(part: &str) -> bool {
    let trimmed = part.trim_end_matches([' ', '.']);
    let cleaned: String = trimmed.chars().filter(|c| !is_format_control(*c)).collect();
    let folded: String = cleaned.to_lowercase();
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"(?i)\A\.?git~[0-9]+\z").expect("static regex"));
    folded == ".git" || re.is_match(&folded)
}

/// Name-equivalence normalization for protected-path compares.
pub fn fs_equiv_path(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .map(|part| {
            let trimmed = part.trim_end_matches([' ', '.']);
            trimmed
                .chars()
                .filter(|c| !is_format_control(*c))
                .collect::<String>()
                .nfkc()
                .collect::<String>()
                .to_lowercase()
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn git_env() -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = GIT_ENV_ALLOWLIST
        .iter()
        .filter_map(|n| std::env::var(n).ok().map(|v| (n.to_string(), v)))
        .collect();
    env.insert("GIT_OPTIONAL_LOCKS".into(), "0".into());
    env
}

fn resolve_git() -> Result<PathBuf> {
    process::which("git")
        .ok_or_else(|| HarnessError::agentic("git binary not found on PATH").detail("looked_for", "git"))
}

/// Recursively delete, clearing read-only bits git leaves on objects.
pub fn rmtree_best_effort(path: &Path) {
    if !path.exists() {
        return;
    }
    for entry in walkdir::WalkDir::new(path).contents_first(true).into_iter().flatten() {
        let p = entry.path();
        if let Ok(meta) = std::fs::symlink_metadata(p) {
            let mut perms = meta.permissions();
            if perms.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(p, perms);
            }
        }
    }
    let _ = std::fs::remove_dir_all(path);
}

impl std::fmt::Debug for RepoWorkspace<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepoWorkspace").field("dest", &self.dest).finish()
    }
}

pub struct RepoWorkspace<'a> {
    dest: PathBuf,
    /// Capability handle on the clone: every read resolves relative to this
    /// open directory, component by component, and cannot leave it.
    dir: cap_std::fs::Dir,
    audit: &'a Audit,
    ctx: &'a AgenticCtx,
    pub max_read_bytes: u64,
    pub max_write_bytes: usize,
}

impl<'a> RepoWorkspace<'a> {
    /// Clone `ctx.acfg.repo` into a fresh `<workspace_root>/<tmp>/repo`.
    pub fn clone(ctx: &'a AgenticCtx) -> Result<Self> {
        let root = &ctx.acfg.deepagent.workspace_root;
        std::fs::create_dir_all(root)?;
        let tmp = tempfile::Builder::new()
            .prefix("cgah-repo-clone-")
            .tempdir_in(root)?
            .keep();
        let dest = tmp.join("repo");
        let cloned = run_read(
            &ctx.audit,
            &ReadRequest {
                op: "repo_clone",
                repo: &ctx.acfg.repo,
                number: None,
                limit: 30,
                min_version: ctx.acfg.gh_min_version,
                timeout_sec: DEFAULT_CLONE_TIMEOUT_SEC,
                retries: ctx.acfg.gh_retries,
                dest: Some(&dest),
            },
        );
        if let Err(e) = cloned {
            rmtree_best_effort(&tmp);
            ctx.audit
                .log(json!({"event": "agentic_repo_workspace_clone_failed", "repo": ctx.acfg.repo}));
            return Err(e);
        }
        if !dest.is_dir() {
            rmtree_best_effort(&tmp);
            return Err(
                HarnessError::agentic("clone produced no repository directory").detail("repo", ctx.acfg.repo.clone())
            );
        }
        ctx.audit.log(json!({"event": "agentic_repo_workspace_cloned", "repo": ctx.acfg.repo, "dest": dest.display().to_string()}));
        let dir = open_jail_dir(&dest)?;
        Ok(Self {
            dest,
            dir,
            audit: &ctx.audit,
            ctx,
            max_read_bytes: DEFAULT_MAX_READ_BYTES,
            max_write_bytes: DEFAULT_MAX_WRITE_BYTES,
        })
    }

    /// Re-open an existing clone; `dest` must be exactly `workspace_root/<tmp>/repo`.
    pub fn attach(ctx: &'a AgenticCtx, dest: &Path) -> Result<Self> {
        let workspace_root = dunce::canonicalize(&ctx.acfg.deepagent.workspace_root)
            .unwrap_or(ctx.acfg.deepagent.workspace_root.clone());
        let dest_resolved = dunce::canonicalize(dest).map_err(|_| {
            HarnessError::agentic("cannot attach: clone directory does not exist")
                .detail("dest", dest.display().to_string())
        })?;
        if !dest_resolved.starts_with(&workspace_root) {
            return Err(
                HarnessError::agentic("attach target is outside the configured workspace root")
                    .detail("dest", dest.display().to_string()),
            );
        }
        let two_up = dest_resolved.parent().and_then(|p| p.parent());
        if two_up != Some(workspace_root.as_path()) {
            return Err(HarnessError::agentic(
                "attach target is not a clone() output (must be workspace_root/<tempdir>/repo)",
            )
            .detail("dest", dest.display().to_string()));
        }
        if !dest_resolved.is_dir() {
            return Err(HarnessError::agentic("cannot attach: clone directory does not exist")
                .detail("dest", dest.display().to_string()));
        }
        let dir = open_jail_dir(&dest_resolved)?;
        Ok(Self {
            dest: dest_resolved,
            dir,
            audit: &ctx.audit,
            ctx,
            max_read_bytes: DEFAULT_MAX_READ_BYTES,
            max_write_bytes: DEFAULT_MAX_WRITE_BYTES,
        })
    }

    /// Test/constructor for an already-populated directory (no clone).
    pub fn open_existing(ctx: &'a AgenticCtx, dest: &Path) -> Result<Self> {
        let dest = dunce::canonicalize(dest)?;
        let dir = open_jail_dir(&dest)?;
        Ok(Self {
            dest,
            dir,
            audit: &ctx.audit,
            ctx,
            max_read_bytes: DEFAULT_MAX_READ_BYTES,
            max_write_bytes: DEFAULT_MAX_WRITE_BYTES,
        })
    }

    pub fn worktree(&self) -> &Path {
        &self.dest
    }

    fn deny(&self, tool: &str, message: &str, target: Option<&str>) -> HarnessError {
        self.audit
            .log(json!({"event": "agentic_repo_workspace_denied", "op": tool, "reason": message, "target": target}));
        let mut e = HarnessError::agentic(message).detail("tool", tool);
        if let Some(t) = target {
            e = e.detail("target", t);
        }
        e
    }

    pub fn require_write(&self, tool: &str, reason: &str, confirm: bool) -> Result<()> {
        super::writer::current_repository_policy(self.ctx, tool, reason, confirm)?;
        Ok(())
    }

    fn dest_resolved(&self) -> PathBuf {
        dunce::canonicalize(&self.dest).unwrap_or_else(|_| self.dest.clone())
    }

    /// Validate one repo-relative path; returns the LANDED relative path.
    fn validate_write_path(&self, tool: &str, target: &str, must_exist: bool) -> Result<String> {
        if target.is_empty() || target.contains('\0') {
            return Err(self.deny(tool, "git path must be a non-empty string", Some(target)));
        }
        let canonical = canonical_repo_path(target).ok_or_else(|| {
            self.deny(
                tool,
                "git path must be a safe relative path inside the clone",
                Some(target),
            )
        })?;
        let parts: Vec<&str> = canonical.split('/').collect();
        if parts.iter().any(|p| is_dotgit_name(p)) {
            return Err(self.deny(tool, "git path may not touch the clone's .git directory", Some(target)));
        }
        let dest_resolved = self.dest_resolved();
        let mut candidate = dest_resolved.clone();
        for p in &parts {
            candidate.push(p);
        }
        let resolved = if must_exist {
            dunce::canonicalize(&candidate).map_err(|e| {
                self.deny(tool, "git path does not exist in the clone", Some(target))
                    .detail("error_type", e.kind().to_string())
            })?
        } else {
            match dunce::canonicalize(&candidate) {
                Ok(r) => r,
                Err(_) => {
                    // A dangling leaf symlink: follow it lexically to where the write LANDS.
                    if let Ok(link) = std::fs::read_link(&candidate) {
                        let base = candidate.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                        let landed = if link.is_absolute() { link } else { base.join(link) };
                        super::config::lexical_normalize(&landed)
                    } else {
                        // Walk up to the nearest existing ancestor and resolve that.
                        let mut ancestor = candidate.clone();
                        let mut tail: Vec<std::ffi::OsString> = Vec::new();
                        loop {
                            match dunce::canonicalize(&ancestor) {
                                Ok(r) => {
                                    let mut out = r;
                                    for t in tail.iter().rev() {
                                        out.push(t);
                                    }
                                    break out;
                                }
                                Err(_) => {
                                    let Some(parent) = ancestor.parent().map(|p| p.to_path_buf()) else {
                                        return Err(self.deny(tool, "git path escaped the clone root", Some(target)));
                                    };
                                    if let Some(name) = ancestor.file_name() {
                                        tail.push(name.to_os_string());
                                    }
                                    ancestor = parent;
                                }
                            }
                        }
                    }
                }
            }
        };
        if !resolved.starts_with(&dest_resolved) {
            return Err(self.deny(tool, "git path escaped the clone root", Some(target)));
        }
        let git_dir = dunce::canonicalize(dest_resolved.join(".git")).unwrap_or_else(|_| dest_resolved.join(".git"));
        if resolved == git_dir || resolved.starts_with(&git_dir) {
            return Err(self.deny(tool, "git path may not touch the clone's .git directory", Some(target)));
        }
        let landed = resolved
            .strip_prefix(&dest_resolved)
            .map_err(|_| self.deny(tool, "git path escaped the clone root", Some(target)))?;
        let rel: Vec<String> = landed
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        Ok(if rel.is_empty() { canonical } else { rel.join("/") })
    }

    fn run_git(&self, tool: &str, args: &[&str], timeout_sec: u64, extra_env: &[(&str, &str)]) -> Result<String> {
        let binary = resolve_git()?;
        let mut argv = vec![binary.display().to_string(), "-c".into(), "core.fsmonitor=false".into()];
        argv.extend(args.iter().map(|a| a.to_string()));
        let mut env = git_env();
        for (k, v) in extra_env {
            env.insert(k.to_string(), v.to_string());
        }
        let out = process::run(RunSpec {
            argv: &argv,
            cwd: Some(&self.dest),
            env: Some(&env),
            timeout: Duration::from_secs(timeout_sec),
            stdin: None,
        })
        .map_err(|e| match e {
            process::ProcessError::Timeout { .. } => {
                self.audit
                    .log(json!({"event": "agentic_repo_workspace_git_failed", "op": tool, "reason": "timeout"}));
                HarnessError::agentic(format!("git {tool} timed out after {timeout_sec}s")).detail("tool", tool)
            }
            process::ProcessError::Spawn(e) => HarnessError::agentic(format!("git {tool} could not start: {e}")),
        })?;
        if out.status != Some(0) {
            self.audit
                .log(json!({"event": "agentic_repo_workspace_git_failed", "op": tool, "exit_code": out.status}));
            return Err(HarnessError::agentic(format!(
                "git {tool} failed with exit code {}",
                out.status.unwrap_or(-1)
            ))
            .detail("tool", tool)
            .detail("exit_code", out.status.unwrap_or(-1))
            .detail(
                "stderr",
                out.stderr
                    .chars()
                    .rev()
                    .take(2000)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>(),
            ));
        }
        self.audit
            .log(json!({"event": "agentic_repo_workspace_git_ok", "op": tool, "exit_code": 0}));
        Ok(out.stdout)
    }

    // ------------------------------------------------------------ reads

    /// Validate a repo-relative read target (never "cleaned"; see `repo_paths`).
    fn read_target(&self, target: &str) -> Result<String> {
        crate::common::repo_paths::canonical_repo_relative_path(target).ok_or_else(|| {
            HarnessError::agentic(format!("cannot read '{target}' from the cloned repository")).detail("target", target)
        })
    }

    fn read_denied(&self, target: &str, why: &str) -> HarnessError {
        self.audit
            .log(json!({"event": "agentic_repo_workspace_denied", "op": "read_file", "target": target, "reason": why}));
        HarnessError::agentic(format!("cannot read '{target}' from the cloned repository"))
            .detail("target", target)
            .detail("error", why)
    }

    /// Read one UTF-8 text file from the clone (bounded).
    ///
    /// Resolution happens inside the capability `Dir`: each path component is
    /// opened relative to the previous one, and a symlink that points outside
    /// the clone fails to resolve rather than being followed. The leaf is opened
    /// with `O_NOFOLLOW` on unix so a leaf symlink is refused outright.
    pub fn read_file(&self, target: &str) -> Result<String> {
        let rel = self.read_target(target).inspect_err(|e| {
            self.audit.log(json!({"event": "agentic_repo_workspace_denied", "op": "read_file", "target": target, "reason": e.message}));
        })?;
        let meta = self
            .dir
            .symlink_metadata(&rel)
            .map_err(|_| self.read_denied(target, "escape or missing"))?;
        if !meta.is_file() {
            return Err(self.read_denied(target, "not a regular file"));
        }
        if meta.len() > self.max_read_bytes {
            return Err(self.read_denied(target, "exceeds max_read_bytes"));
        }
        let data = open_nofollow(&self.dir, &rel).map_err(|_| self.read_denied(target, "escape or missing"))?;
        if data.len() as u64 > self.max_read_bytes {
            return Err(self.read_denied(target, "exceeds max_read_bytes"));
        }
        self.audit
            .log(json!({"event": "agentic_repo_workspace_read", "op": "read_file", "target": target}));
        String::from_utf8(data)
            .map_err(|_| HarnessError::agentic(format!("'{target}' is not valid UTF-8 text")).detail("target", target))
    }

    /// Stat one path (existence + kind) without reading it.
    pub fn stat_file(&self, target: &str) -> Result<Value> {
        let rel = self.read_target(target)?;
        let meta = self
            .dir
            .metadata(&rel)
            .map_err(|_| HarnessError::agentic(format!("cannot stat '{target}'")).detail("target", target))?;
        self.audit
            .log(json!({"event": "agentic_repo_workspace_read", "op": "stat_file", "target": target}));
        Ok(json!({"path": target, "is_file": meta.is_file(), "is_dir": meta.is_dir(), "size": meta.len()}))
    }

    // ------------------------------------------------------------ writes

    pub fn checkout_branch(&self, name: &str, reason: &str, confirm: bool) -> Result<Value> {
        let tool = "checkout_branch";
        self.require_write(tool, reason, confirm)?;
        let id = identity::identity()?;
        if !id.branch_is_valid(name) {
            return Err(self.deny(
                tool,
                &format!(
                    "branch name must start with one of [{}] and use only [A-Za-z0-9._/-] after the slash",
                    id.allowed_prefixes_help()
                ),
                Some(name),
            ));
        }
        self.run_git(tool, &["checkout", "-b", name], DEFAULT_GIT_WRITE_TIMEOUT_SEC, &[])?;
        self.audit
            .log(json!({"event": "agentic_repo_workspace_git_op", "op": tool, "branch": name}));
        Ok(json!({"branch": name}))
    }

    /// Write one text file (create or overwrite) inside the clone.
    pub fn write_file(&self, target: &str, content: &str, reason: &str, confirm: bool) -> Result<Value> {
        let tool = "write_file";
        self.require_write(tool, reason, confirm)?;
        let encoded = content.as_bytes();
        if encoded.len() > self.max_write_bytes {
            return Err(self
                .deny(tool, "write content exceeds max_write_bytes", Some(target))
                .detail("bytes", encoded.len() as u64));
        }
        let relative = self.validate_write_path(tool, target, false)?;
        for path in [target, &relative] {
            if super::real_repo_loop::matches_protected_path(path, &self.ctx.acfg.deepagent.protected_write_paths) {
                return Err(self.deny(tool, "protected write destination", Some(path)));
            }
        }
        let path = self.dest.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        let written = (|| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, encoded)
        })();
        if let Err(e) = written {
            return Err(self
                .deny(tool, "git path is not writable in the clone", Some(target))
                .detail("error_type", e.kind().to_string()));
        }
        self.audit.log(json!({"event": "agentic_repo_workspace_write", "target": relative, "bytes": encoded.len(), "sha256": crate::common::sha256_bytes_hex(encoded)}));
        Ok(json!({"target": relative, "bytes": encoded.len()}))
    }

    /// Validate the whole proposal, stage replacements through retained parent
    /// capabilities, then rename. Ordinary failures roll back installed files.
    /// This is not a crash-atomic multi-file filesystem transaction.
    pub fn apply_proposal(
        &self,
        proposal: &super::edits::Proposal,
        protected: &[String],
        reason: &str,
        confirm: bool,
    ) -> Result<Vec<String>> {
        let tool = "apply_proposal";
        self.require_write(tool, reason, confirm)?;
        let mut targets = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (raw, content) in &proposal.files {
            let landed = self.validate_write_path(tool, raw, false)?;
            for path in [raw, &landed] {
                if super::real_repo_loop::matches_protected_path(path, protected)
                    || super::real_repo_loop::matches_protected_path(
                        path,
                        &self.ctx.acfg.deepagent.protected_write_paths,
                    )
                {
                    return Err(self.deny(tool, "protected write destination", Some(path)));
                }
            }
            if !seen.insert(fs_equiv_path(&landed)) {
                return Err(self.deny(tool, "duplicate landed destination", Some(raw)));
            }
            if content.len() > self.max_write_bytes {
                return Err(self.deny(tool, "write content exceeds max_write_bytes", Some(raw)));
            }
            let expected = proposal
                .originals
                .get(raw)
                .ok_or_else(|| self.deny(tool, "missing original precondition", Some(raw)))?;
            // Even in-jail symlink aliases are refused for proposal writes.
            // Checked before the stale/unseen gate so a leaf symlink is denied
            // as a symlink, not as an unseen regular file.
            let canonical = canonical_repo_path(raw).ok_or_else(|| self.deny(tool, "unsafe destination", Some(raw)))?;
            let mut prefix = PathBuf::new();
            for part in Path::new(&canonical).components() {
                prefix.push(part);
                if self
                    .dir
                    .symlink_metadata(&prefix)
                    .is_ok_and(|m| m.file_type().is_symlink())
                {
                    return Err(self.deny(tool, "symlink proposal destinations are refused", Some(raw)));
                }
            }
            let actual = self.read_file(raw).ok();
            if &actual != expected || (expected.is_none() && self.dir.symlink_metadata(raw).is_ok()) {
                return Err(self.deny(tool, "stale or unseen existing file; select current context", Some(raw)));
            }
            targets.push((landed, expected.clone(), content.clone()));
        }
        let total: usize = targets.iter().map(|(_, _, content)| content.len()).sum();
        if total as u64 > self.ctx.acfg.deepagent.max_write_budget_bytes {
            return Err(self.deny(tool, "aggregate write budget exceeded", None));
        }
        let mut staged = Vec::new();
        for (path, original, content) in &targets {
            let (parent, leaf) = self.proposal_parent(path)?;
            staged.push(StagedReplacement::prepare(parent, leaf, original.as_deref(), content)?);
        }
        let apply = (|| -> Result<()> {
            for file in &mut staged {
                self.require_write(tool, reason, confirm)?;
                file.install()?;
            }
            Ok(())
        })();
        if let Err(error) = apply {
            for file in staged.iter_mut().rev() {
                file.rollback().map_err(|_| rollback_quarantine_error())?;
            }
            return Err(error);
        }
        for file in &mut staged {
            // Successful batch: backups may now be removed. On failed rollback
            // installed remains true, preserving the recovery preimage.
            file.installed = false;
        }
        let paths: Vec<_> = targets.into_iter().map(|(path, _, _)| path).collect();
        self.audit
            .log(json!({"event":"agentic_proposal_applied", "paths": paths, "bytes": total}));
        Ok(paths)
    }

    fn proposal_parent(&self, path: &str) -> Result<(cap_std::fs::Dir, String)> {
        let mut parts: Vec<_> = path.split('/').collect();
        let leaf = parts
            .pop()
            .ok_or_else(|| self.deny("apply_proposal", "empty destination", Some(path)))?;
        let mut parent = self.dir.try_clone()?;
        for part in parts {
            match parent.create_dir(part) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                let mut options = cap_std::fs::OpenOptions::new();
                options.read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
                parent = cap_std::fs::Dir::from_std_file(parent.open_with(part, &options)?.into_std());
            }
            #[cfg(not(unix))]
            {
                if parent.symlink_metadata(part)?.file_type().is_symlink() {
                    return Err(self.deny("apply_proposal", "symlink parent refused", Some(path)));
                }
                parent = parent.open_dir(part)?;
            }
        }
        Ok((parent, leaf.to_string()))
    }

    pub fn add(&self, paths: &[String], reason: &str, confirm: bool) -> Result<Value> {
        let tool = "add";
        self.require_write(tool, reason, confirm)?;
        if paths.is_empty() {
            return Err(self.deny(tool, "git add requires at least one path", None));
        }
        let mut validated = Vec::new();
        for p in paths {
            validated.push(self.validate_write_path(tool, p, true)?);
        }
        let mut args = vec!["add", "--"];
        args.extend(validated.iter().map(|s| s.as_str()));
        self.run_git(tool, &args, DEFAULT_GIT_WRITE_TIMEOUT_SEC, &[])?;
        self.audit
            .log(json!({"event": "agentic_repo_workspace_git_op", "op": tool, "paths": validated}));
        Ok(json!({"paths": validated}))
    }

    /// Commit staged changes with the configured committer identity, `--no-verify`.
    pub fn commit(&self, message: &str, reason: &str, confirm: bool) -> Result<Value> {
        let tool = "commit";
        self.require_write(tool, reason, confirm)?;
        if message.trim().is_empty() {
            return Err(self.deny(tool, "commit message must be a non-empty string", None));
        }
        let id = identity::identity()?;
        let email = format!("user.email={}", id.commit_email);
        let name = format!("user.name={}", id.commit_name);
        self.run_git(
            tool,
            &["-c", &email, "-c", &name, "commit", "--no-verify", "-m", message],
            DEFAULT_GIT_WRITE_TIMEOUT_SEC,
            &[],
        )?;
        self.audit.log(json!({"event": "agentic_repo_workspace_git_op", "op": tool, "message_sha256": crate::common::sha256_hex(message)}));
        Ok(json!({}))
    }

    /// Push one agent branch to origin. The only network write here; no credential of its own.
    pub fn push_branch(&self, name: &str, reason: &str, confirm: bool) -> Result<Value> {
        let tool = "push_branch";
        self.require_write(tool, reason, confirm)?;
        let id = identity::identity()?;
        if !id.branch_is_valid(name) {
            return Err(self.deny(
                tool,
                &format!(
                    "push branch must start with one of [{}] and use only [A-Za-z0-9._/-] after the slash",
                    id.allowed_prefixes_help()
                ),
                Some(name),
            ));
        }
        self.run_git(
            tool,
            &["push", "--set-upstream", "origin", "--", name],
            DEFAULT_PUSH_TIMEOUT_SEC,
            &[("GIT_TERMINAL_PROMPT", "0")],
        )?;
        self.audit
            .log(json!({"event": "agentic_repo_workspace_git_op", "op": tool, "branch": name}));
        Ok(json!({"pushed": name}))
    }

    pub fn diff(&self, cached: bool) -> Result<String> {
        let tool = "diff";
        let args: &[&str] = if cached {
            &["diff", "--no-ext-diff", "--no-textconv", "--cached"]
        } else {
            &["diff", "--no-ext-diff", "--no-textconv"]
        };
        let out = self.run_git(tool, args, DEFAULT_GIT_WRITE_TIMEOUT_SEC, &[])?;
        self.audit
            .log(json!({"event": "agentic_repo_workspace_git_op", "op": tool, "cached": cached, "bytes": out.len()}));
        Ok(out)
    }

    pub fn untracked_files(&self) -> Result<Vec<String>> {
        let tool = "status";
        let out = self.run_git(
            tool,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            DEFAULT_GIT_WRITE_TIMEOUT_SEC,
            &[],
        )?;
        let paths: Vec<String> = out
            .split('\0')
            .filter_map(|r| r.strip_prefix("?? ").map(|s| s.to_string()))
            .collect();
        self.audit
            .log(json!({"event": "agentic_repo_workspace_git_op", "op": tool, "count": paths.len()}));
        Ok(paths)
    }

    /// Retain the clone (no held handles to release in this port).
    pub fn release(&self) {}

    /// Delete the clone (its parent temp directory) from disk.
    pub fn close(&self) {
        if let Some(parent) = self.dest.parent() {
            rmtree_best_effort(parent);
        }
    }

    /// Production dispose after `run_real_repo_loop`. A failed rollback keeps
    /// the clone and `.cgah-backup-*` preimages for later discard.
    pub fn close_after_loop(&self, error: Option<&HarnessError>) {
        if error.is_some_and(is_proposal_rollback_quarantine) {
            self.release();
            return;
        }
        self.close();
    }
}

/// True when proposal application aborted because rollback itself failed.
pub fn is_proposal_rollback_quarantine(error: &HarnessError) -> bool {
    error.message.contains("rollback failed")
        || error
            .details
            .get("quarantine")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

fn rollback_quarantine_error() -> HarnessError {
    HarnessError::agentic("proposal rollback failed; candidate is quarantined and must be discarded")
        .detail("quarantine", true)
}

/// One staged replacement and its recovery preimage.
struct StagedReplacement {
    parent: cap_std::fs::Dir,
    leaf: String,
    staged: String,
    backup: String,
    original: Option<Vec<u8>>,
    replacement: Vec<u8>,
    installed: bool,
}

impl StagedReplacement {
    fn prepare(parent: cap_std::fs::Dir, leaf: String, original: Option<&str>, content: &str) -> Result<Self> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let result = Self {
            parent,
            leaf,
            staged: format!(".cgah-stage-{id}"),
            backup: format!(".cgah-backup-{id}"),
            original: original.map(|s| s.as_bytes().to_vec()),
            replacement: content.as_bytes().to_vec(),
            installed: false,
        };
        for (name, data) in [
            (&result.staged, Some(result.replacement.as_slice())),
            (&result.backup, result.original.as_deref()),
        ] {
            if let Some(data) = data {
                use std::io::Write;
                let mut opts = cap_std::fs::OpenOptions::new();
                opts.write(true).create_new(true);
                let mut file = result.parent.open_with(name, &opts)?;
                file.write_all(data)?;
                if result.original.is_some() {
                    file.set_permissions(result.parent.metadata(&result.leaf)?.permissions())?;
                }
                file.sync_all()?;
            }
        }
        Ok(result)
    }

    fn current(&self) -> std::io::Result<Option<Vec<u8>>> {
        match open_nofollow(&self.parent, &self.leaf) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn install(&mut self) -> Result<()> {
        if self.current()? != self.original {
            return Err(HarnessError::agentic(
                "stale file at application boundary; rolling back proposal",
            ));
        }
        self.parent.rename(&self.staged, &self.parent, &self.leaf)?;
        self.installed = true;
        #[cfg(test)]
        after_install_hook(&self.leaf);
        Ok(())
    }

    fn rollback(&mut self) -> Result<()> {
        if self.installed {
            if self.current()?.as_deref() != Some(self.replacement.as_slice()) {
                return Err(HarnessError::agentic("concurrent change prevents safe rollback"));
            }
            if self.original.is_some() {
                self.parent.rename(&self.backup, &self.parent, &self.leaf)?;
            } else {
                self.parent.remove_file(&self.leaf)?;
            }
            self.installed = false;
        }
        Ok(())
    }
}

impl Drop for StagedReplacement {
    fn drop(&mut self) {
        let _ = self.parent.remove_file(&self.staged);
        if !self.installed {
            let _ = self.parent.remove_file(&self.backup);
        }
    }
}

fn open_jail_dir(dest: &Path) -> Result<cap_std::fs::Dir> {
    cap_std::fs::Dir::open_ambient_dir(dest, cap_std::ambient_authority()).map_err(|e| {
        HarnessError::agentic(format!("cannot open clone directory: {e}")).detail("dest", dest.display().to_string())
    })
}

#[cfg(unix)]
fn open_nofollow(dir: &cap_std::fs::Dir, rel: &str) -> std::io::Result<Vec<u8>> {
    use cap_std::fs::OpenOptionsExt;
    use std::io::Read;
    let mut opts = cap_std::fs::OpenOptions::new();
    opts.read(true).custom_flags(libc::O_NOFOLLOW);
    let mut f = dir.open_with(rel, &opts)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(not(unix))]
fn open_nofollow(dir: &cap_std::fs::Dir, rel: &str) -> std::io::Result<Vec<u8>> {
    dir.read(rel)
}

#[cfg(test)]
thread_local! {
    static AFTER_INSTALL: std::cell::Cell<Option<fn(&str)>> = const { std::cell::Cell::new(None) };
    static TAMPER: std::cell::RefCell<Option<(std::path::PathBuf, String, String)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn after_install_hook(leaf: &str) {
    AFTER_INSTALL.with(|h| {
        if let Some(hook) = h.get() {
            hook(leaf);
        }
    });
}

#[cfg(test)]
fn tamper_after_first_install(leaf: &str) {
    TAMPER.with(|t| {
        let state = t.borrow();
        let Some((root, first, second)) = state.as_ref() else {
            return;
        };
        if leaf == first {
            let _ = std::fs::write(root.join(first), "tampered-first\n");
            let _ = std::fs::write(root.join(second), "tampered-second\n");
        }
    });
}

#[cfg(test)]
mod transaction_tests {
    use super::*;
    use crate::agentic::ctx::AgenticCtx;
    use crate::agentic::edits::Proposal;
    use crate::common::config::AppConfig;
    use std::collections::BTreeMap;

    fn write_enabled_ctx(dir: &Path) -> AgenticCtx {
        let text = AppConfig::embedded_default()
            .replacen(
                "enabled: false                 # master switch",
                "enabled: true                  # master switch",
                1,
            )
            .replacen(
                "    enabled: false\n    provider:",
                "    enabled: true\n    provider:",
                1,
            )
            .replacen("allow_git_write_tools: false", "allow_git_write_tools: true", 1);
        let path = dir.join("config.yaml");
        std::fs::write(&path, &text).unwrap();
        AgenticCtx::new(AppConfig::from_str(&text, &path).unwrap(), &path).unwrap()
    }

    #[test]
    fn failed_second_install_rolls_back_first_without_overwriting_concurrent_change() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), "original a").unwrap();
        std::fs::write(root.path().join("b"), "original b").unwrap();
        let parent = open_jail_dir(root.path()).unwrap();
        let mut first =
            StagedReplacement::prepare(parent.try_clone().unwrap(), "a".into(), Some("original a"), "new a").unwrap();
        let mut second = StagedReplacement::prepare(parent, "b".into(), Some("original b"), "new b").unwrap();
        first.install().unwrap();
        assert_eq!(std::fs::read_to_string(root.path().join("a")).unwrap(), "new a");
        std::fs::write(root.path().join("b"), "concurrent b").unwrap();
        assert!(second.install().unwrap_err().message.contains("stale"));
        second.rollback().unwrap();
        first.rollback().unwrap();
        drop((first, second));
        assert_eq!(std::fs::read_to_string(root.path().join("a")).unwrap(), "original a");
        assert_eq!(std::fs::read_to_string(root.path().join("b")).unwrap(), "concurrent b");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }

    #[test]
    fn failed_rollback_keeps_recovery_backup() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), "original a").unwrap();
        let parent = open_jail_dir(root.path()).unwrap();
        let mut first = StagedReplacement::prepare(parent, "a".into(), Some("original a"), "new a").unwrap();
        first.install().unwrap();
        std::fs::write(root.path().join("a"), "tampered a").unwrap();
        assert!(first.rollback().unwrap_err().message.contains("concurrent change"));
        drop(first);
        assert_eq!(std::fs::read_to_string(root.path().join("a")).unwrap(), "tampered a");
        assert!(
            std::fs::read_dir(root.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().starts_with(".cgah-backup-")),
            "failed rollback must retain the recovery backup"
        );
    }

    #[test]
    fn production_error_path_retains_clone_and_backup_on_rollback_failure() {
        let home = tempfile::tempdir().unwrap();
        let ctx = write_enabled_ctx(home.path());
        let holder = home.path().join("holder");
        let clone = holder.join("repo");
        std::fs::create_dir_all(&clone).unwrap();
        std::fs::write(clone.join("a.rs"), "old a\n").unwrap();
        std::fs::write(clone.join("b.rs"), "old b\n").unwrap();
        let ws = RepoWorkspace::open_existing(&ctx, &clone).unwrap();
        TAMPER.with(|t| {
            *t.borrow_mut() = Some((clone.clone(), "a.rs".into(), "b.rs".into()));
        });
        AFTER_INSTALL.with(|h| h.set(Some(tamper_after_first_install)));
        let mut files = BTreeMap::new();
        files.insert("a.rs".into(), "new a\n".into());
        files.insert("b.rs".into(), "new b\n".into());
        let mut originals = BTreeMap::new();
        originals.insert("a.rs".into(), Some("old a\n".into()));
        originals.insert("b.rs".into(), Some("old b\n".into()));
        let err = ws
            .apply_proposal(&Proposal { files, originals }, &[], "test", true)
            .unwrap_err();
        AFTER_INSTALL.with(|h| h.set(None));
        TAMPER.with(|t| *t.borrow_mut() = None);
        assert!(is_proposal_rollback_quarantine(&err), "{}", err.message);
        assert!(
            walkdir::WalkDir::new(&clone)
                .into_iter()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(".cgah-backup-")),
            "recovery backup must remain after rollback failure"
        );
        ws.close_after_loop(Some(&err));
        assert!(clone.is_dir(), "production dispose must not rmtree a quarantined clone");
        assert!(
            walkdir::WalkDir::new(&clone)
                .into_iter()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(".cgah-backup-")),
            "close_after_loop must leave .cgah-backup-* in place"
        );
    }

    #[test]
    fn ordinary_loop_error_still_deletes_the_clone() {
        let home = tempfile::tempdir().unwrap();
        let ctx = write_enabled_ctx(home.path());
        let holder = home.path().join("holder");
        let clone = holder.join("repo");
        std::fs::create_dir_all(&clone).unwrap();
        std::fs::write(clone.join("keep.txt"), "x").unwrap();
        let ws = RepoWorkspace::open_existing(&ctx, &clone).unwrap();
        ws.close_after_loop(Some(&HarnessError::agentic("verification failed")));
        assert!(!clone.exists());
        assert!(!holder.exists());
    }
}
