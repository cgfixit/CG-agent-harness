//! Read-only registry / tools / skills views for `/registry`, `/tools`, `/skills`.
//! Port of `harness/{registry_view,tools_view,skills_view}.py`.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Value};

use super::agent_policy;
use super::prompts::DISCIPLINE_SKILLS;
use crate::common::home::Home;

const DESC_CAP: usize = 72;

fn frontmatter_field(text: &str, key: &str) -> String {
    for line in text.lines().take(40) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(&format!("{key}:")) {
            return rest.trim().trim_matches(['\'', '"', '>', '|', '-']).trim().to_string();
        }
    }
    String::new()
}

/// Skills from `<home>/skills/*/SKILL.md` frontmatter.
pub fn list_repo_skills(skills_dir: &Path) -> Vec<Value> {
    let mut dirs: Vec<_> = std::fs::read_dir(skills_dir)
        .map(|rd| rd.flatten().collect())
        .unwrap_or_default();
    dirs.sort_by_key(|e| e.file_name());
    let mut out = Vec::new();
    for entry in dirs {
        let path = entry.path().join("SKILL.md");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let name = frontmatter_field(&text, "name");
        out.push(json!({
            "name": if name.is_empty() { dir_name } else { name },
            "description": frontmatter_field(&text, "description"),
            "source": "repo",
            "path": format!("skills/{}/SKILL.md", entry.file_name().to_string_lossy()),
        }));
    }
    out
}

/// Entries in the governed registry (read-only). Mapping or list shapes tolerated.
pub fn list_governed_skills(registry_path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(registry_path) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let entries = parsed.get("skills").cloned().unwrap_or(parsed);
    let items: Vec<Value> = match entries {
        Value::Object(map) => map.values().cloned().collect(),
        Value::Array(list) => list,
        _ => Vec::new(),
    };
    items
        .iter()
        .filter_map(|e| {
            let name = e.get("name").and_then(|v| v.as_str())?;
            Some(json!({
                "name": name,
                "description": e.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                "source": "agentic-registry",
                "path": registry_path.display().to_string(),
            }))
        })
        .collect()
}

fn connectors() -> Vec<Value> {
    vec![
        json!({"id": "github", "name": "GitHub (agentic layer)", "kind": "built-in",
               "auth": "gh CLI login (repo writes) / none needed for public reads",
               "driver": "cgagentharness agentic", "notes": "PR/issue context, governed skills registry; read mode by default."}),
        json!({"id": "ollama", "name": "Ollama (local models)", "kind": "built-in",
               "auth": "none (loopback)", "driver": "direct HTTP 127.0.0.1:11434",
               "notes": "Default chat backend; free, offline, no account."}),
        json!({"id": "github-public", "name": "GitHub public API", "kind": "catalog",
               "auth": "none for public data (rate-limited)", "driver": "", "notes": "api.github.com unauthenticated reads."}),
        json!({"id": "openai-compatible", "name": "Any OpenAI-compatible local server", "kind": "catalog",
               "auth": "optional api_key", "driver": "", "notes": "agentic provider 'openai_compatible' (e.g. LM Studio compat)."}),
    ]
}

pub fn full_registry(home: &Home) -> Value {
    let mut skills = list_repo_skills(&home.skills_dir());
    skills.extend(list_governed_skills(&home.registry_path()));
    json!({"skills": skills, "tools": [], "connectors": connectors()})
}

// ---------------------------------------------------------------- tools

/// Paths must match the router templates in `routes/mod.rs` exactly.
const HARNESS_SURFACES: [(&str, &str, &str, &str, &str); 26] = [
    ("chat", "(plain text)", "POST", "/api/chat", "local model chat turn"),
    (
        "goal",
        "/goal",
        "POST",
        "/api/sessions/{session_id}/goal",
        "session-scoped operator intent (not a write authorization)",
    ),
    (
        "loop",
        "/loop",
        "POST",
        "/api/chat",
        "human-gated chat turns toward /goal; never starts /api/agent/*",
    ),
    (
        "cancel",
        "/loop stop",
        "POST",
        "/api/chat/cancel",
        "abort the in-flight model request",
    ),
    (
        "session",
        "/session",
        "GET",
        "/api/sessions",
        "list, create, switch, or rename sessions",
    ),
    (
        "soul",
        "/soul",
        "GET",
        "/api/soul",
        "harness-local soul-in-prompt toggle (does not write soul.md)",
    ),
    (
        "memory",
        "/memory",
        "GET",
        "/api/memory",
        "operator notes in prompt (off by default; not RAG memory/)",
    ),
    (
        "memory-add",
        "/memory add",
        "POST",
        "/api/memory/add",
        "pin one injection-scanned operator note",
    ),
    ("model", "/model", "POST", "/api/model", "select the local chat model"),
    (
        "status",
        "/status",
        "GET",
        "/api/status",
        "harness health, layout, and token tally",
    ),
    (
        "registry",
        "/registry",
        "GET",
        "/api/registry",
        "merged skills / connectors",
    ),
    (
        "github",
        "/github",
        "GET",
        "/api/github/status",
        "read-only agentic GitHub status (subprocess)",
    ),
    (
        "agent-checks",
        "/agent checks",
        "GET",
        "/api/agent/checks",
        "named verification profiles (no subprocess)",
    ),
    (
        "agent-run",
        "/agent confirm",
        "POST",
        "/api/agent/run",
        "human-gated real-repo run (reason + confirm required)",
    ),
    (
        "agent-status",
        "/agent status",
        "GET",
        "/api/agent/runs/{run_id}",
        "inspect a coding-agent run record",
    ),
    (
        "agent-decision",
        "/agent approve",
        "POST",
        "/api/agent/runs/{run_id}/decision",
        "approve or reject a pending run (reaches a git write)",
    ),
    (
        "agent-push",
        "/agent push",
        "POST",
        "/api/agent/runs/{run_id}/push",
        "push an approved branch (disarmed by default)",
    ),
    (
        "agent-publish",
        "/agent publish",
        "POST",
        "/api/agent/runs/{run_id}/publish",
        "open a draft PR (disarmed by default; reason required)",
    ),
    (
        "agent-discard",
        "/agent discard",
        "POST",
        "/api/agent/runs/{run_id}/discard",
        "reclaim the clone of a decided run",
    ),
    (
        "harness",
        "/harness",
        "GET",
        "/api/harness/runs",
        "local harness-optimizer run listing",
    ),
    (
        "tools",
        "/tools",
        "GET",
        "/api/tools",
        "this inventory - wired-tool diagram",
    ),
    (
        "skills",
        "/skills",
        "GET",
        "/api/skills",
        "wired skill diagram (prompt + agent-check)",
    ),
    (
        "web",
        "/web",
        "GET",
        "/api/web",
        "allowlist-only web fetch (off until /web on)",
    ),
    (
        "web-fetch",
        "/web fetch",
        "POST",
        "/api/web/fetch",
        "GET one allowlisted URL; no crawl, no search engine",
    ),
    (
        "web-search",
        "/web search",
        "POST",
        "/api/web/search",
        "grep allowlisted pages for a query (no search engine)",
    ),
    (
        "keys",
        "/api",
        "GET",
        "/api/keys",
        "managed credential status (masked tail only, never a value)",
    ),
];

fn tree_pair(name: &str, wired: bool, head_tail: &str, detail: &str, last: bool, indent: &str) -> (String, String) {
    let branch = if last { "└─" } else { "├─" };
    let mark = if wired { "●" } else { "○" };
    let child = if last { "  " } else { "│ " };
    (
        format!("{indent}{branch}[{name}] {mark}  {head_tail}"),
        format!("{indent}{child} {detail}"),
    )
}

fn box_render(title: &str, inner: &[String]) -> String {
    let width = inner.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let edge: String = "─".repeat(width + 1);
    let title = format!(" /{title} ");
    let prefix = if title.chars().count() < edge.chars().count() {
        title
    } else {
        String::new()
    };
    let rest: String = edge.chars().skip(prefix.chars().count()).collect();
    let mut lines = vec![format!("┌{prefix}{rest}┐")];
    for l in inner {
        let pad = width - l.chars().count();
        lines.push(format!("│{l}{} │", " ".repeat(pad)));
    }
    lines.push(format!("└{edge}┘"));
    lines.join("\n")
}

fn render_tools_diagram(tools: &[Value], wired: usize, total: usize) -> String {
    if tools.len() == 1 {
        let r = &tools[0];
        let inner = vec![
            format!(" name        {}", r["name"].as_str().unwrap_or("")),
            format!(" slash       {}", r["slash"].as_str().unwrap_or("")),
            format!(
                " route       {} {}",
                r["method"].as_str().unwrap_or(""),
                r["path"].as_str().unwrap_or("")
            ),
            format!(" kind        {}", r["kind"].as_str().unwrap_or("")),
            format!(
                " invoked     {}",
                if r["invoked"].as_bool().unwrap_or(false) {
                    "yes"
                } else {
                    "no (catalog only)"
                }
            ),
            format!(
                " wired       {}",
                if r["wired"].as_bool().unwrap_or(false) {
                    "yes"
                } else {
                    "no"
                }
            ),
            String::new(),
            format!(" {}", r["description"].as_str().unwrap_or("")),
        ];
        return box_render(r["name"].as_str().unwrap_or(""), &inner);
    }
    let mut lines = vec![format!("HARNESS TOOLS — {wired} wired / {total} listed"), String::new()];
    if tools.is_empty() {
        lines.push("(none)".into());
        return lines.join("\n");
    }
    lines.push("console".into());
    let last_index = tools.len() - 1;
    for (i, r) in tools.iter().enumerate() {
        let (head, detail) = tree_pair(
            r["name"].as_str().unwrap_or(""),
            r["wired"].as_bool().unwrap_or(false),
            &format!(
                "{} {}",
                r["method"].as_str().unwrap_or(""),
                r["path"].as_str().unwrap_or("")
            ),
            &format!(
                "{:<16} {}",
                r["slash"].as_str().unwrap_or(""),
                r["description"].as_str().unwrap_or("")
            ),
            i == last_index,
            "",
        );
        lines.push(head);
        lines.push(detail);
    }
    lines.join("\n")
}

pub fn list_wired_tools(registered: &BTreeSet<String>) -> Value {
    let tools: Vec<Value> = HARNESS_SURFACES
        .iter()
        .map(|(name, slash, method, path, desc)| {
            json!({"name": name, "slash": slash, "method": method, "path": path, "description": desc,
                   "kind": "harness", "invoked": true, "wired": registered.contains(*path)})
        })
        .collect();
    let wired: Vec<Value> = tools
        .iter()
        .filter(|t| t["wired"].as_bool().unwrap_or(false))
        .cloned()
        .collect();
    let count = wired.len();
    json!({"tools": tools, "wired": count, "total": HARNESS_SURFACES.len(),
           "diagram": render_tools_diagram(&wired, count, HARNESS_SURFACES.len())})
}

// ---------------------------------------------------------------- skills

fn clip_desc(text: &str) -> String {
    let cleaned: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= DESC_CAP {
        cleaned
    } else {
        format!("{}…", crate::common::clip_chars(&cleaned, DESC_CAP - 1))
    }
}

fn render_skills_diagram(rows: &[Value], wired: usize, total: usize) -> String {
    if rows.len() == 1 {
        let r = &rows[0];
        let inner = vec![
            format!(" name        {}", r["name"].as_str().unwrap_or("")),
            format!(" role        {}", r["role"].as_str().unwrap_or("")),
            format!(" path        {}", r["path"].as_str().unwrap_or("")),
            format!(" source      {}", r["source"].as_str().unwrap_or("")),
            format!(
                " invoked     {}",
                if r["invoked"].as_bool().unwrap_or(false) {
                    "yes"
                } else {
                    "no (catalog only)"
                }
            ),
            format!(
                " wired       {}",
                if r["wired"].as_bool().unwrap_or(false) {
                    "yes"
                } else {
                    "no"
                }
            ),
            String::new(),
            format!(" {}", r["description"].as_str().unwrap_or("")),
        ];
        return box_render(r["name"].as_str().unwrap_or(""), &inner);
    }
    let mut lines = vec![
        format!("HARNESS SKILLS — {wired} wired / {total} listed"),
        String::new(),
    ];
    let groups = [
        ("prompt", "prompt (injected into every chat turn)"),
        ("check", "agent-check (named /agent checks profiles)"),
        ("repo", "repo catalog (present; this console does not invoke these)"),
        (
            "governed",
            "governed registry (read-only; mutations stay behind agentic CLI)",
        ),
    ];
    let mut any = false;
    for (role, heading) in groups {
        let subset: Vec<&Value> = rows.iter().filter(|r| r["role"].as_str() == Some(role)).collect();
        if subset.is_empty() {
            continue;
        }
        any = true;
        lines.push(heading.into());
        let last = subset.len() - 1;
        for (i, r) in subset.iter().enumerate() {
            let (head, detail) = tree_pair(
                r["name"].as_str().unwrap_or(""),
                r["wired"].as_bool().unwrap_or(false),
                r["path"].as_str().unwrap_or(""),
                r["description"].as_str().unwrap_or(""),
                i == last,
                "",
            );
            lines.push(head);
            lines.push(detail);
        }
    }
    if !any {
        lines.push("(none)".into());
    }
    lines.join("\n")
}

pub fn list_wired_skills(home: &Home) -> Value {
    let repo_entries = list_repo_skills(&home.skills_dir());
    let mut rows: Vec<Value> = Vec::new();
    for entry in &repo_entries {
        let name = entry["name"].as_str().unwrap_or("").to_string();
        let path = entry["path"].as_str().unwrap_or("").to_string();
        let desc = clip_desc(entry["description"].as_str().unwrap_or(""));
        if DISCIPLINE_SKILLS.contains(&name.as_str()) {
            let readable = home.root.join(&path).is_file();
            rows.push(
                json!({"name": name, "role": "prompt", "path": path, "description": desc, "source": "repo",
                             "invoked": readable, "wired": readable}),
            );
        } else {
            rows.push(
                json!({"name": name, "role": "repo", "path": path, "description": desc, "source": "repo",
                             "invoked": false, "wired": false}),
            );
        }
    }
    // Check profiles are fixed commands (not skill scripts) in this port, so they
    // are listed as wired agent-checks with no path.
    for (name, desc) in agent_policy::available_profiles() {
        rows.push(
            json!({"name": name, "role": "check", "path": "", "description": clip_desc(&desc),
                         "source": "agent-check", "invoked": true, "wired": true}),
        );
    }
    for entry in list_governed_skills(&home.registry_path()) {
        rows.push(json!({"name": entry["name"], "role": "governed", "path": entry["path"],
                         "description": clip_desc(entry["description"].as_str().unwrap_or("")),
                         "source": "agentic-registry", "invoked": false, "wired": false}));
    }
    let wired: Vec<Value> = rows
        .iter()
        .filter(|r| r["wired"].as_bool().unwrap_or(false))
        .cloned()
        .collect();
    let count = wired.len();
    let total = rows.len();
    json!({"skills": rows, "wired": count, "total": total, "diagram": render_skills_diagram(&wired, count, total)})
}
