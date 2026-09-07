//! Deterministic governance checks over candidate text, port of
//! `agentic/harness_optimizer/governance.py`: the injection scan (prose) and
//! the code-shape scan (exfiltration / obfuscated exec / reverse shell /
//! pipe-to-shell), every code-shape rule a COMBINATION, never a single token.

use std::sync::OnceLock;

use regex::RegexBuilder;

use crate::common::injection::Scanner;

pub const CRITICAL_SEVERITY: &str = "critical";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceFinding {
    pub severity: String,
    pub code: String,
    pub message: String,
}

impl GovernanceFinding {
    pub fn critical(code: &str, message: &str) -> Self {
        Self {
            severity: CRITICAL_SEVERITY.into(),
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn as_gate_string(&self) -> String {
        format!("{}: {}: {}", self.severity, self.code, self.message)
    }
}

fn re(pattern: &str) -> regex::Regex {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .expect("static regex")
}

struct CodeShapeRules {
    secret_path: regex::Regex,
    egress: regex::Regex,
    decode: regex::Regex,
    dynamic_exec: regex::Regex,
    fd_dup: regex::Regex,
    shell_path: regex::Regex,
    socket: regex::Regex,
    pipe_to_shell: regex::Regex,
}

fn rules() -> &'static CodeShapeRules {
    static R: OnceLock<CodeShapeRules> = OnceLock::new();
    R.get_or_init(|| CodeShapeRules {
        secret_path: re(r"\.ssh/|id_rsa|id_ed25519|id_ecdsa|\.aws/credentials|\.netrc|\.git-credentials|\.env\b|credentials\.json|\.kube/config"),
        egress: re(r"\bsubprocess\b|\bos\.system\b|\bos\.popen\b|\bsocket\b|\brequests\.|\burllib\b|\bhttpx\b|\bcurl\b|\bwget\b|\bftplib\b|\bsmtplib\b|\breqwest\b|\bstd::process::Command\b"),
        decode: re(r"b64decode|b32decode|a85decode|bytes\.fromhex|codecs\.decode|zlib\.decompress|gzip\.decompress|marshal\.loads|pickle\.loads|base64::decode|STANDARD\.decode"),
        dynamic_exec: re(r"\bexec\s*\(|\beval\s*\(|\bcompile\s*\(|__import__\s*\("),
        fd_dup: re(r"\bdup2\b|\bpty\.spawn\b"),
        shell_path: re(r"/bin/sh|/bin/bash|/bin/zsh|\bcmd\.exe\b|\bpowershell\b"),
        socket: re(r"\bsocket\b|\bTcpStream\b"),
        pipe_to_shell: re(r"(curl|wget)[^\n|]{0,200}\|\s*(sudo\s+)?(ba|z|d)?sh\b"),
    })
}

/// Flag malicious-CODE shapes in content a planner proposes writing to a repo.
pub fn inspect_code_shape(candidate_text: &str, enabled: bool) -> Vec<GovernanceFinding> {
    if !enabled || candidate_text.is_empty() {
        return Vec::new();
    }
    let r = rules();
    let hits = [
        (
            r.secret_path.is_match(candidate_text) && r.egress.is_match(candidate_text),
            "candidate_credential_egress",
            "reads a credential/private-key path AND carries an egress or exec mechanism",
        ),
        (
            r.decode.is_match(candidate_text) && r.dynamic_exec.is_match(candidate_text),
            "candidate_obfuscated_exec",
            "decodes an encoded blob AND passes it to a dynamic exec/eval",
        ),
        (
            r.socket.is_match(candidate_text)
                && (r.fd_dup.is_match(candidate_text) || r.shell_path.is_match(candidate_text)),
            "candidate_reverse_shell",
            "opens a socket AND duplicates file descriptors or spawns a shell",
        ),
        (
            r.pipe_to_shell.is_match(candidate_text),
            "candidate_pipe_to_shell",
            "pipes a network download directly into a shell",
        ),
    ];
    hits.iter()
        .filter(|(hit, _, _)| *hit)
        .map(|(_, code, msg)| GovernanceFinding::critical(code, &format!("proposed file content {msg}")))
        .collect()
}

/// Flag prompt-injection-shaped candidate content (one critical finding on the first hit).
pub fn inspect_candidate_text(scanner: &Scanner, candidate_text: &str) -> Vec<GovernanceFinding> {
    if scanner.scan(candidate_text).is_empty() {
        Vec::new()
    } else {
        vec![GovernanceFinding::critical(
            "candidate_injection_pattern",
            "candidate content matches a governed injection pattern",
        )]
    }
}
