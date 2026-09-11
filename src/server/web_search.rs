//! Allowlist-only web fetch for `/web`, port of `harness/web_search.py`.
//!
//! Default-off. A fetch happens only if the tool is enabled, the URL matches
//! the operator allowlist, and DNS resolves to a public address BEFORE the
//! GET. The GET target is rebuilt from the persisted allowlist row only, so no
//! user-supplied URL fragment reaches the socket. Empty allowlist is fail-closed.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::common::atomic::{write_atomic, write_json_atomic};
use crate::common::audit::Audit;
use crate::common::errors::{HarnessError, Result};
use crate::common::tool_broker::assert_allowed;

pub const MAX_ALLOW: usize = 32;
pub const MAX_BYTES: usize = 262_144;
const TIMEOUT_SEC: f64 = 8.0;
const MAX_QUERY: usize = 200;
const MAX_SNIPPET: usize = 160;
const MAX_HITS_PER_URL: usize = 3;
const MAX_SEARCH_URLS: usize = 8;
const MAX_CONTEXT: usize = 4000;
const MAX_RAW: usize = 500;
const SNIP_BEFORE: usize = 40;
const SNIP_AFTER: usize = 120;
const BLOCKED_HOSTS: [&str; 4] = [
    "localhost",
    "localhost.localdomain",
    "metadata.google.internal",
    "metadata.goog",
];
const PATH_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._~/=+-";
const TEXT_TYPES: [&str; 5] = [
    "text/",
    "application/json",
    "application/xml",
    "application/xhtml+xml",
    "application/javascript",
];
const SKIP_TAGS: [&str; 4] = ["script", "style", "noscript", "template"];
const BREAK_TAGS: [&str; 10] = ["p", "div", "br", "li", "tr", "h1", "h2", "h3", "h4", "pre"];

fn werr(code: &str, message: impl Into<String>) -> HarnessError {
    HarnessError::new(code, message)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowEntry {
    pub scheme: String,
    pub host: String,
    pub port: String,
    pub path: String,
    pub raw: String,
}

impl AllowEntry {
    fn key(&self) -> (String, String, String) {
        (self.host.clone(), self.port.clone(), self.path.clone())
    }

    fn to_json(&self) -> Value {
        json!({"scheme": self.scheme, "host": self.host, "port": self.port, "path": self.path, "raw": self.raw})
    }

    fn rendered(&self) -> String {
        if self.raw.is_empty() {
            format!("{}://{}{}", self.scheme, authority(&self.host, &self.port), self.path)
        } else {
            self.raw.clone()
        }
    }
}

fn host_of(raw: &str) -> String {
    raw.trim().to_lowercase().trim_end_matches('.').to_string()
}

fn authority(host: &str, port: &str) -> String {
    let h = match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => format!("[{host}]"),
        _ => host.to_string(),
    };
    if port.is_empty() {
        h
    } else {
        format!("{h}:{port}")
    }
}

/// Hand-rolled `is_global` (the std one is unstable).
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let seg = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || seg[0] == 0x2001 && seg[1] == 0x0db8 // documentation
                || seg[0] == 0x2001 && seg[1] == 0x0002 && seg[2] == 0 // benchmarking
                || v6 == Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 1))
        }
    }
}

fn is_public_v4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    !(v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_unspecified()
        || v4.is_multicast()
        || (o[0] == 100 && (64..=127).contains(&o[1])) // CGNAT 100.64/10
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // benchmarking
        || o[0] == 0
        || o[0] >= 240) // reserved + 255.255.255.255
}

/// Normalise a host or URL into an allowlist row. No DNS (offline-safe).
pub fn parse_allow_entry(raw: &str) -> Result<AllowEntry> {
    let text = raw.trim();
    if text.is_empty() || text.chars().count() > MAX_RAW {
        return Err(werr("WEB_BAD_URL", "allowlist entry must be a non-empty URL or host"));
    }
    let text = if text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let parsed = url::Url::parse(&text).map_err(|_| werr("WEB_BAD_URL", "URL could not be parsed"))?;
    let scheme = parsed.scheme().to_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(werr("WEB_BAD_URL", "only http and https URLs are allowed"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(werr("WEB_BAD_URL", "URLs with userinfo are refused"));
    }
    let port = match parsed.port() {
        None => String::new(),
        Some(p) if (scheme == "http" && p == 80) || (scheme == "https" && p == 443) => String::new(),
        Some(p) => p.to_string(),
    };
    let host = host_of(parsed.host_str().unwrap_or(""))
        .trim_matches(['[', ']'])
        .to_string();
    if host.is_empty() || BLOCKED_HOSTS.contains(&host.as_str()) || host.ends_with(".local") {
        return Err(werr("WEB_SSRF_DENIED", "that host cannot be allowlisted"));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if !is_public_ip(ip) {
            return Err(werr("WEB_SSRF_DENIED", "private or loopback IPs cannot be allowlisted"));
        }
    }
    let mut path = parsed.path().to_string();
    if path.is_empty() {
        path = "/".into();
    }
    if !path.starts_with('/') {
        path = format!("/{path}");
    }
    let raw = format!("{scheme}://{}{path}", authority(&host, &port));
    Ok(AllowEntry {
        scheme,
        host,
        port,
        path,
        raw,
    })
}

fn load_entries(path: &Path) -> Result<Vec<AllowEntry>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(werr("WEB_ALLOWLIST_UNREADABLE", "allowlist is unreadable")),
    };
    let parsed: Value =
        serde_json::from_str(&text).map_err(|_| werr("WEB_ALLOWLIST_UNREADABLE", "allowlist is unreadable"))?;
    let rows = parsed.get("entries").cloned().unwrap_or(parsed);
    let mut out = Vec::new();
    if let Some(list) = rows.as_array() {
        for row in list {
            let host = row.get("host").and_then(|v| v.as_str()).unwrap_or("");
            if host.is_empty() {
                continue;
            }
            out.push(AllowEntry {
                scheme: row
                    .get("scheme")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("https")
                    .to_string(),
                host: host_of(host),
                port: row.get("port").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                path: row
                    .get("path")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("/")
                    .to_string(),
                raw: row.get("raw").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            });
        }
    }
    Ok(out)
}

fn save_entries(path: &Path, entries: &[AllowEntry]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let rows: Vec<Value> = entries.iter().map(|e| e.to_json()).collect();
    write_json_atomic(path, &json!({"entries": rows}))
}

/// The matching allowlist row, or None.
pub fn url_is_allowed<'a>(url: &str, entries: &'a [AllowEntry]) -> Option<&'a AllowEntry> {
    let wanted = parse_allow_entry(url).ok()?;
    let mut aliases: BTreeSet<String> = BTreeSet::new();
    aliases.insert(wanted.host.clone());
    if let Some(bare) = wanted.host.strip_prefix("www.") {
        aliases.insert(bare.to_string());
    } else {
        aliases.insert(format!("www.{}", wanted.host));
    }
    for entry in entries {
        if !aliases.contains(&entry.host) || entry.port != wanted.port {
            continue;
        }
        let prefix = if entry.path.is_empty() {
            "/"
        } else {
            entry.path.as_str()
        };
        if prefix == "/" {
            return Some(entry);
        }
        let trimmed = prefix.trim_end_matches('/');
        if wanted.path == trimmed || wanted.path.starts_with(&format!("{trimmed}/")) {
            return Some(entry);
        }
    }
    None
}

/// Resolve `host` and refuse any non-public address (pre-connect snapshot; not a pin).
pub async fn assert_public_host(host: &str) -> Result<()> {
    let clean = host_of(host);
    if clean.is_empty() || BLOCKED_HOSTS.contains(&clean.as_str()) {
        return Err(werr("WEB_SSRF_DENIED", "host is not fetchable"));
    }
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((clean.as_str(), 443))
        .await
        .map_err(|_| werr("WEB_DNS", format!("DNS failed for {clean}")))?
        .collect();
    if addrs.is_empty() {
        return Err(werr("WEB_DNS", format!("DNS returned no addresses for {clean}")));
    }
    for a in addrs {
        if !is_public_ip(a.ip()) {
            return Err(
                werr("WEB_SSRF_DENIED", "resolved address is not public; refused").detail("host", clean.clone())
            );
        }
    }
    Ok(())
}

/// Visible text from HTML (script/style dropped); otherwise a whitespace-collapsed body.
pub fn extract_text(body: &str, content_type: &str) -> String {
    let lowered = content_type.split(';').next().unwrap_or("").trim().to_lowercase();
    let text = if lowered.contains("html") {
        strip_html(body)
    } else {
        body.to_string()
    };
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::common::clip_chars(&unescape_entities(&collapsed), MAX_BYTES)
}

fn strip_html(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut skip_depth = 0usize;
    let mut rest = body;
    while let Some(start) = rest.find('<') {
        if skip_depth == 0 {
            out.push_str(&rest[..start]);
        }
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            break;
        };
        let tag_body = &after[..end];
        let is_close = tag_body.starts_with('/');
        let name: String = tag_body
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        if SKIP_TAGS.contains(&name.as_str()) {
            if is_close {
                skip_depth = skip_depth.saturating_sub(1);
            } else if !tag_body.ends_with('/') {
                skip_depth += 1;
            }
        } else if BREAK_TAGS.contains(&name.as_str()) && skip_depth == 0 {
            out.push('\n');
        }
        rest = &after[end + 1..];
    }
    if skip_depth == 0 {
        out.push_str(rest);
    }
    out
}

fn unescape_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find('&') {
        out.push_str(&rest[..idx]);
        let tail = &rest[idx..];
        let Some(semi) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let entity = &tail[1..semi];
        let replacement = match entity {
            "amp" => Some("&".to_string()),
            "lt" => Some("<".to_string()),
            "gt" => Some(">".to_string()),
            "quot" => Some("\"".to_string()),
            "apos" | "#39" => Some("'".to_string()),
            "nbsp" => Some(" ".to_string()),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32)
                .map(|c| c.to_string()),
            e if e.starts_with('#') => e[1..]
                .parse::<u32>()
                .ok()
                .and_then(char::from_u32)
                .map(|c| c.to_string()),
            _ => None,
        };
        match replacement {
            Some(r) if semi <= 12 => {
                out.push_str(&r);
                rest = &tail[semi + 1..];
            }
            _ => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn snippets(text: &str, query: &regex::Regex) -> Vec<String> {
    let mut found = Vec::new();
    for matched in query.find_iter(text).take(MAX_HITS_PER_URL) {
        let lo = floor_char(text, matched.start().saturating_sub(SNIP_BEFORE));
        let hi = floor_char(text, (matched.end() + SNIP_AFTER).min(text.len()));
        let mut chunk = text[lo..hi].trim().to_string();
        if lo > 0 {
            chunk = format!("…{chunk}");
        }
        if hi < text.len() {
            chunk = format!("{chunk}…");
        }
        found.push(crate::common::clip_chars(&chunk, MAX_SNIPPET));
    }
    found
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// GET URL from the persisted allowlist row only, never from user URL pieces.
fn allowlist_target(entry: &AllowEntry) -> Result<String> {
    let scheme = if entry.scheme == "http" || entry.scheme == "https" {
        entry.scheme.as_str()
    } else {
        "https"
    };
    if !entry.port.is_empty() {
        let ok = entry.port.bytes().all(|b| b.is_ascii_digit())
            && entry.port.parse::<u32>().map(|p| p > 0 && p <= 65535).unwrap_or(false);
        if !ok {
            return Err(werr("WEB_BAD_URL", "allowlist port is invalid"));
        }
    }
    let mut path = if entry.path.is_empty() {
        "/".to_string()
    } else {
        entry.path.clone()
    };
    if !path.starts_with('/') {
        path = format!("/{path}");
    }
    if path.chars().any(|c| !PATH_CHARS.contains(c)) {
        return Err(werr("WEB_BAD_URL", "allowlist path is invalid"));
    }
    Ok(format!("{scheme}://{}{path}", authority(&entry.host, &entry.port)))
}

pub struct WebTool {
    tools_dir: PathBuf,
    /// Test hook: `(host, addr)` skips the public-DNS check for `host` and pins it to `addr`.
    pub test_resolve: Option<(String, SocketAddr)>,
}

impl std::fmt::Debug for WebTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebTool").field("tools_dir", &self.tools_dir).finish()
    }
}

impl WebTool {
    pub fn new(tools_dir: &Path) -> Self {
        Self {
            tools_dir: tools_dir.to_path_buf(),
            test_resolve: None,
        }
    }

    fn allow_path(&self) -> PathBuf {
        self.tools_dir.join("web_allowlist.json")
    }
    fn last_path(&self) -> PathBuf {
        self.tools_dir.join("web_last.json")
    }
    fn context_path(&self) -> PathBuf {
        self.tools_dir.join("web_context.txt")
    }

    pub fn status(&self, enabled: bool) -> Result<Value> {
        let entries = load_entries(&self.allow_path())?;
        let ctx = self.context_path();
        let injected = std::fs::metadata(&ctx)
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false);
        Ok(json!({
            "enabled": enabled,
            "allowlist": entries.iter().map(|e| e.rendered()).collect::<Vec<_>>(),
            "injected": injected,
            "has_last": self.last_path().is_file(),
            "max_allow": MAX_ALLOW,
        }))
    }

    pub fn allow(&self, raw: &str, enabled: bool) -> Result<Value> {
        let entry = parse_allow_entry(raw)?;
        let path = self.allow_path();
        let mut entries = load_entries(&path)?;
        if entries.iter().any(|e| e.key() == entry.key()) {
            return self.status(enabled);
        }
        if entries.len() >= MAX_ALLOW {
            return Err(werr("WEB_ALLOWLIST_FULL", format!("allowlist cap is {MAX_ALLOW}")));
        }
        entries.push(entry);
        save_entries(&path, &entries)?;
        self.status(enabled)
    }

    pub fn deny(&self, raw: &str, enabled: bool) -> Result<Value> {
        let entry = parse_allow_entry(raw)?;
        let path = self.allow_path();
        let entries: Vec<AllowEntry> = load_entries(&path)?
            .into_iter()
            .filter(|e| e.key() != entry.key())
            .collect();
        save_entries(&path, &entries)?;
        self.status(enabled)
    }

    pub fn context_text(&self) -> String {
        std::fs::read_to_string(self.context_path())
            .map(|t| crate::common::clip_chars(&t, MAX_CONTEXT))
            .unwrap_or_default()
    }

    pub fn forget(&self, enabled: bool) -> Result<Value> {
        let _ = std::fs::remove_file(self.context_path());
        let _ = std::fs::remove_file(self.last_path());
        self.status(enabled)
    }

    pub fn inject(&self, enabled: bool) -> Result<Value> {
        let payload: Value = std::fs::read_to_string(self.last_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .ok_or_else(|| werr("WEB_NO_LAST", "nothing to inject - /web fetch or /web search first"))?;
        let text = payload
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let source = payload.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if text.is_empty() {
            return Err(werr("WEB_NO_LAST", "last extract is empty"));
        }
        let body = crate::common::clip_chars(&format!("Source: {source}\n\n{text}"), MAX_CONTEXT);
        write_atomic(&self.context_path(), body.as_bytes(), None)?;
        let mut status = self.status(enabled)?;
        status["chars"] = json!(body.chars().count());
        Ok(status)
    }

    fn require_enabled(&self, enabled: bool) -> Result<Vec<AllowEntry>> {
        if !enabled {
            return Err(werr(
                "WEB_DISABLED",
                "web fetch is off - /web on after allowlisting hosts",
            ));
        }
        let entries = load_entries(&self.allow_path())?;
        if entries.is_empty() {
            return Err(werr(
                "WEB_ALLOWLIST_EMPTY",
                "allowlist is empty - /web allow <url> first (fail-closed)",
            ));
        }
        Ok(entries)
    }

    fn client(&self) -> Result<reqwest::Client> {
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs_f64(TIMEOUT_SEC))
            .user_agent("CGagentHarness-web/0.1 (+allowlist-only; no-browser)");
        if let Some((host, addr)) = &self.test_resolve {
            builder = builder.resolve(host, *addr);
        }
        builder.build().map_err(|_| werr("WEB_FETCH_FAILED", "fetch failed"))
    }

    fn gate_tool(&self, name: &str, argv: &[String], enabled: bool, audit: &Audit) -> Result<()> {
        let allow: BTreeSet<String> = if enabled {
            ["web_fetch".to_string(), "web_search".to_string()]
                .into_iter()
                .collect()
        } else {
            BTreeSet::new()
        };
        assert_allowed(name, argv, &allow, audit)
            .map(|_| ())
            .map_err(|e| HarnessError::new("WEB_TOOL_DENIED", e.message).with_details(e.details))
    }

    async fn get(&self, url: &str, entries: &[AllowEntry]) -> Result<Value> {
        let entry = url_is_allowed(url, entries)
            .ok_or_else(|| werr("WEB_HOST_DENIED", "URL is not on the allowlist").detail("url", url))?;
        let full = if url.contains("://") {
            url.to_string()
        } else {
            format!("https://{url}")
        };
        if let Ok(parsed) = url::Url::parse(&full) {
            if parsed.query().is_some() || parsed.fragment().is_some() {
                return Err(werr("WEB_BAD_URL", "query or fragment is not allowed"));
            }
        }
        match &self.test_resolve {
            Some((host, _)) if host == &entry.host => {}
            _ => assert_public_host(&entry.host).await?,
        }
        let target = allowlist_target(entry)?;
        let client = self.client()?;
        let resp = client
            .get(&target)
            .send()
            .await
            .map_err(|_| werr("WEB_FETCH_FAILED", "fetch failed"))?;
        let status = resp.status().as_u16();
        if status >= 400 {
            return Err(werr("WEB_FETCH_FAILED", format!("upstream HTTP {status}")).detail("status", status));
        }
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/plain")
            .to_string();
        let lowered = ctype.to_lowercase();
        let is_text = TEXT_TYPES.iter().any(|p| lowered.starts_with(p)) || lowered.contains("html");
        if !is_text {
            return Err(werr("WEB_NOT_TEXT", "content-type is not text; refused"));
        }
        let mut body: Vec<u8> = Vec::new();
        let mut resp = resp;
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|_| werr("WEB_FETCH_FAILED", "fetch failed"))?
        {
            let remaining = MAX_BYTES.saturating_sub(body.len());
            body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            if body.len() >= MAX_BYTES {
                break;
            }
        }
        let text = extract_text(&String::from_utf8_lossy(&body), &ctype);
        Ok(json!({"url": target, "status": status, "content_type": ctype, "chars": text.chars().count(), "text": text}))
    }

    pub async fn fetch(&self, url: &str, enabled: bool, audit: &Audit) -> Result<Value> {
        let target = url.trim().to_string();
        let entries = self.require_enabled(enabled)?;
        self.gate_tool("web_fetch", std::slice::from_ref(&target), enabled, audit)?;
        let page = self.get(&target, &entries).await?;
        write_json_atomic(&self.last_path(), &page)?;
        Ok(page)
    }

    pub async fn search(&self, query: &str, enabled: bool, audit: &Audit) -> Result<Value> {
        let needle = query.trim().to_string();
        if needle.is_empty() || needle.chars().count() > MAX_QUERY {
            return Err(werr("WEB_BAD_QUERY", "search query must be 1-200 characters"));
        }
        let entries: Vec<AllowEntry> = self
            .require_enabled(enabled)?
            .into_iter()
            .take(MAX_SEARCH_URLS)
            .collect();
        self.gate_tool("web_search", std::slice::from_ref(&needle), enabled, audit)?;
        // Match original-text offsets; Unicode case conversion can change byte lengths.
        let query_pattern = regex::RegexBuilder::new(&regex::escape(&needle))
            .case_insensitive(true)
            .build()
            .map_err(|_| werr("WEB_BAD_QUERY", "search query could not be compiled"))?;
        let mut hits = Vec::new();
        let mut errors = Vec::new();
        let mut recorded_last = false;
        for entry in &entries {
            let url = entry.rendered();
            match self.get(&url, std::slice::from_ref(entry)).await {
                Ok(page) => {
                    let text = page.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    let snips = snippets(text, &query_pattern);
                    if !snips.is_empty() {
                        hits.push(json!({"url": page["url"], "snippets": snips}));
                        if !recorded_last {
                            write_json_atomic(&self.last_path(), &page)?;
                            recorded_last = true;
                        }
                    }
                }
                Err(e) => {
                    tracing::info!("web search skipped {url}: {}", e.code);
                    errors.push(json!({"url": url, "code": e.code}));
                }
            }
        }
        Ok(json!({"query": needle, "hits": hits, "errors": errors, "scanned": entries.len()}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_snippets_preserve_matches_after_unicode_case_changes() {
        let query = regex::RegexBuilder::new("target")
            .case_insensitive(true)
            .build()
            .unwrap();
        for prefix in ["K", "İ"] {
            let text = format!("{} Target", prefix.repeat(200));
            let hits = snippets(&text, &query);
            assert_eq!(hits.len(), 1);
            assert!(hits[0].contains("Target"), "match missing after {prefix}: {hits:?}");
            assert!(hits[0].chars().count() <= MAX_SNIPPET);
        }
        assert_eq!(snippets("Target TARGET target target", &query).len(), MAX_HITS_PER_URL);
    }
}
