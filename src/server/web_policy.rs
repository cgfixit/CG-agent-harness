//! Content permission only. Account and provider authority are separate gates.
use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::common::errors::{HarnessError, Result};

pub const MAX_POLICY_BYTES: u64 = 65_536;
pub const MAX_RULES: usize = 32;
pub const MAX_URL_BYTES: usize = 2048;

pub fn error(code: &str, message: &str) -> HarnessError {
    HarnessError::new(code, message)
}

/// Canonical fetch identity. Reject ambiguous input before URL parsing erases it.
pub fn canonical_url(raw: &str) -> Result<Url> {
    let bad = || {
        error(
            "WEB_BAD_URL",
            "use an explicit, unambiguous HTTP(S) URL without credentials",
        )
    };
    let text = raw.trim();
    if text.is_empty()
        || text.len() > MAX_URL_BYTES
        || text.chars().any(|c| c.is_control() || c.is_whitespace())
        || text.contains(['\\', '*'])
    {
        return Err(bad());
    }
    let (scheme, rest) = text.split_once("://").ok_or_else(bad)?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") || rest.starts_with('/') {
        return Err(bad());
    }
    let authority = rest.split(['/', '?', '#']).next().ok_or_else(bad)?;
    if authority.contains(['@', '%']) || authority.is_empty() || authority.ends_with(':') {
        return Err(bad());
    }
    let path = rest
        .strip_prefix(authority)
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    if path.split('/').any(|s| matches!(s, "." | "..")) {
        return Err(bad());
    }
    // Percent escapes retain their identity (including query order). Refuse
    // nested escapes and encoded dot/separator/space bytes in paths. Query data
    // may encode these characters; it cannot change the parsed URL authority.
    // Encoded controls and malformed escapes are refused everywhere.
    let mut normalized = String::with_capacity(text.len());
    let mut chars = text.chars();
    let mut query = false;
    while let Some(c) = chars.next() {
        if c == '?' {
            query = true;
        }
        if c == '#' {
            query = false;
        }
        normalized.push(c);
        if c == '%' {
            let a = chars.next().and_then(|x| x.to_digit(16)).ok_or_else(bad)?;
            let b = chars.next().and_then(|x| x.to_digit(16)).ok_or_else(bad)?;
            let byte = a * 16 + b;
            if byte < 32 || byte == 127 || !query && matches!(byte, 32 | 37 | 46 | 47 | 92) {
                return Err(bad());
            }
            normalized.push_str(&format!("{byte:02X}"));
        }
    }
    let mut url = Url::parse(&normalized).map_err(|_| bad())?;
    if !url.username().is_empty() || url.password().is_some() || url.port() == Some(0) {
        return Err(bad());
    }
    let host = url.host_str().ok_or_else(bad)?;
    if host.ends_with('.') || host.contains("..") {
        return Err(bad());
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) if !is_public_ip(ip.into()) => {
            return Err(error("WEB_SSRF_DENIED", "non-public address refused"))
        }
        Some(url::Host::Ipv6(ip)) if !is_public_ip(ip.into()) => {
            return Err(error("WEB_SSRF_DENIED", "non-public address refused"))
        }
        Some(url::Host::Domain(host))
            if !host.contains('.')
                || [
                    "localhost",
                    "local",
                    "internal",
                    "localhost.localdomain",
                    "metadata.goog",
                ]
                .iter()
                .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}"))) =>
        {
            return Err(error("WEB_SSRF_DENIED", "local or metadata host refused"))
        }
        _ => {}
    }
    // WHATWG parsing accepts nonstandard IPv4 such as 127.1 or hexadecimal.
    // Refuse these alternate authority spellings, even for public addresses.
    if let Some(url::Host::Ipv4(ip)) = url.host() {
        if authority.split(':').next() != Some(ip.to_string().as_str()) {
            return Err(bad());
        }
    }
    url.set_fragment(None);
    Ok(url)
}

/// Conservative global-unicast subset; reject transition and special-use ranges.
pub fn is_public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let b = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_documentation()
                || b[0] == 0
                || b[0] >= 224
                || b[0] == 100 && (64..=127).contains(&b[1])
                || b[0] == 192 && (b[1] == 0 && b[2] == 0 || b[1] == 88 && b[2] == 99)
                || b[0] == 198 && matches!(b[1], 18 | 19))
        }
        std::net::IpAddr::V6(ip) => {
            let s = ip.segments();
            // 2000::/3 only; exclude special-purpose 2001::/23, docs and 6to4.
            s[0] & 0xe000 == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8)
                    || s[0] == 0x2002
                    || s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

pub fn valid_group(group: &str) -> bool {
    !group.is_empty()
        && group.len() <= 48
        && group
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub pattern: String,
    #[serde(default = "default_group")]
    pub group: String,
    #[serde(default)]
    pub seeds: Vec<String>,
}

fn default_group() -> String {
    "default".into()
}

#[derive(Debug)]
struct Pattern {
    url: Url,
    subdomains: bool,
    subtree: bool,
}

impl Pattern {
    fn parse(raw: &str) -> Result<Self> {
        let bad = || {
            error(
                "WEB_BAD_URL",
                "wildcards require an explicit host or /path/*; wildcard queries are refused",
            )
        };
        let raw = raw.trim();
        let (scheme, rest) = raw.split_once("://").ok_or_else(bad)?;
        let subdomains = rest.starts_with("*.");
        let rest = rest.strip_prefix("*.").unwrap_or(rest);
        let subtree = rest.ends_with("/*");
        let rest = if subtree { &rest[..rest.len() - 1] } else { rest };
        let url = canonical_url(&format!("{scheme}://{rest}"))?;
        if (subdomains || subtree) && (url.query().is_some() || raw.contains('#')) {
            return Err(bad());
        }
        if subdomains && !matches!(url.host(), Some(url::Host::Domain(d)) if d.contains('.')) {
            return Err(bad());
        }
        Ok(Self {
            url,
            subdomains,
            subtree,
        })
    }

    fn rendered(&self) -> String {
        let mut url = self.url.to_string();
        if self.subdomains {
            url.insert_str(url.find("://").unwrap() + 3, "*.");
        }
        if self.subtree {
            url.push('*');
        }
        url
    }

    fn permits(&self, target: &Url) -> bool {
        if self.url.scheme() != target.scheme() || self.url.port_or_known_default() != target.port_or_known_default() {
            return false;
        }
        let host = target.host_str().unwrap_or("");
        let allowed_host = self.url.host_str().unwrap_or("");
        let host_ok = if self.subdomains {
            host != allowed_host && host.ends_with(&format!(".{allowed_host}"))
        } else {
            host == allowed_host
        };
        host_ok
            && if self.subtree {
                target.path().starts_with(self.url.path())
            } else {
                target.path() == self.url.path() && target.query() == self.url.query()
            }
    }
}

impl Rule {
    pub fn new(pattern: &str, group: &str, seeds: &[String]) -> Result<Self> {
        let parsed = Pattern::parse(pattern)?;
        let pattern = parsed.rendered();
        let rule = Self {
            id: crate::common::sha256_hex(&format!("{group}\n{pattern}"))[..32].into(),
            pattern,
            group: group.into(),
            seeds: seeds.to_vec(),
        };
        rule.validate()?;
        Ok(rule)
    }

    fn validate(&self) -> Result<()> {
        if self.id.len() != 32
            || !self.id.bytes().all(|c| c.is_ascii_hexdigit())
            || !valid_group(&self.group)
            || self.seeds.len() > 16
        {
            return Err(error("WEB_POLICY_INVALID", "invalid rule ID, group, or seed count"));
        }
        let pattern = Pattern::parse(&self.pattern)?;
        for seed in &self.seeds {
            if !pattern.permits(&canonical_url(seed)?) {
                return Err(error("WEB_POLICY_INVALID", "seed is outside its rule"));
            }
        }
        Ok(())
    }

    pub fn permits(&self, target: &Url) -> bool {
        Pattern::parse(&self.pattern).is_ok_and(|p| p.permits(target))
    }

    pub fn start_urls(&self) -> Vec<String> {
        if !self.seeds.is_empty() {
            return self.seeds.clone();
        }
        match Pattern::parse(&self.pattern) {
            Ok(p) if !p.subdomains && !p.subtree => vec![p.url.to_string()],
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub version: u32,
    pub rules: Vec<Rule>,
    #[serde(skip)]
    pub revision: String,
}

impl Policy {
    pub fn empty() -> Self {
        Self {
            version: 1,
            rules: Vec::new(),
            revision: String::new(),
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let bad = || {
            error(
                "WEB_POLICY_INVALID",
                "policy must be a supported, valid, bounded rule document",
            )
        };
        if bytes.len() as u64 > MAX_POLICY_BYTES {
            return Err(bad());
        }
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| bad())?;
        let mut policy = if value.get("version").is_some() {
            serde_json::from_value::<Self>(value).map_err(|_| bad())?
        } else {
            Self::legacy(value)?
        };
        if policy.version != 1 || policy.rules.len() > MAX_RULES {
            return Err(bad());
        }
        let mut ids = BTreeSet::new();
        for rule in &policy.rules {
            rule.validate()?;
            if !ids.insert(&rule.id) {
                return Err(bad());
            }
        }
        policy.revision = crate::common::sha256_bytes_hex(bytes);
        Ok(policy)
    }

    fn legacy(value: serde_json::Value) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Legacy {
            scheme: String,
            host: String,
            port: String,
            path: String,
            raw: String,
        }
        let bad = || {
            error(
                "WEB_POLICY_INVALID",
                "legacy policy is malformed; preserve it and correct it manually",
            )
        };
        let rows = if value.is_array() {
            value
        } else {
            let map = value.as_object().ok_or_else(bad)?;
            if map.len() != 1 {
                return Err(bad());
            }
            map.get("entries").cloned().ok_or_else(bad)?
        };
        let rows: Vec<Legacy> = serde_json::from_value(rows).map_err(|_| bad())?;
        if rows.len() > MAX_RULES {
            return Err(bad());
        }
        let mut policy = Self::empty();
        for row in rows {
            // The old network target used stored components, never raw aliases.
            // Reject rows the old target builder could not fetch; never widen.
            if !matches!(row.scheme.as_str(), "http" | "https")
                || row.host.is_empty()
                || row
                    .path
                    .chars()
                    .any(|c| !"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._~/=+-".contains(c))
                || (!row.port.is_empty() && row.port.parse::<u16>().ok().filter(|p| *p > 0).is_none())
            {
                return Err(bad());
            }
            let _ = row.raw;
            let host = if row.host.parse::<std::net::Ipv6Addr>().is_ok() {
                format!("[{}]", row.host)
            } else {
                row.host
            };
            let port = if row.port.is_empty() {
                String::new()
            } else {
                format!(":{}", row.port)
            };
            let path = if row.path.starts_with('/') {
                row.path
            } else {
                format!("/{}", row.path)
            };
            let rule = Rule::new(&format!("{}://{host}{port}{path}", row.scheme), "default", &[])?;
            if !policy.rules.iter().any(|r| r.id == rule.id) {
                policy.rules.push(rule);
            }
        }
        Ok(policy)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
        }
        let file = options.open(path).map_err(|_| {
            error(
                "WEB_POLICY_UNAVAILABLE",
                "policy is missing or unreadable; web access refused",
            )
        })?;
        if !file.metadata()?.is_file() {
            return Err(error("WEB_POLICY_INVALID", "policy must be a regular file"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_POLICY_BYTES + 1).read_to_end(&mut bytes)?;
        Self::parse(&bytes)
    }

    pub fn permits(&self, target: &Url, group: Option<&str>) -> bool {
        self.rules
            .iter()
            .any(|r| group.is_none_or(|g| r.group == g) && r.permits(target))
    }

    pub fn authorize(&self, raw: &str, group: Option<&str>) -> Result<Url> {
        let target = canonical_url(raw)?;
        if !self.permits(&target, group) {
            return Err(error("WEB_HOST_DENIED", "URL is not permitted by current policy"));
        }
        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_explicit_wildcards_preserve_authority_and_query_boundaries() {
        let rule = Rule::new("https://example.com/article?q=1#section", "docs", &[]).unwrap();
        assert!(rule.permits(&canonical_url("https://EXAMPLE.com:443/article?q=1#other").unwrap()));
        for bad in [
            "http://example.com/article?q=1",
            "https://www.example.com/article?q=1",
            "https://example.com/article",
            "https://example.com/article/child?q=1",
            "https://example.com:444/article?q=1",
        ] {
            assert!(!rule.permits(&canonical_url(bad).unwrap()), "{bad}");
        }
        for (pattern, yes, no) in [
            (
                "https://example.com/docs/*",
                "https://example.com/docs/",
                "https://example.com/docs",
            ),
            (
                "https://example.com/*",
                "https://example.com/nested/page?q=1",
                "https://www.example.com/nested/page?q=1",
            ),
            (
                "https://*.example.com/docs/*",
                "https://a.b.example.com/docs/page",
                "https://example.com/docs/page",
            ),
        ] {
            let rule = Rule::new(pattern, "docs", &[]).unwrap();
            assert!(rule.permits(&canonical_url(yes).unwrap()));
            assert!(!rule.permits(&canonical_url(no).unwrap()));
            assert!(!rule.permits(&canonical_url("https://example.com.evil.test/docs/page").unwrap()));
            if pattern.contains("/docs/") {
                assert!(!rule.permits(&canonical_url("https://example.com/docs-other/page").unwrap()));
            }
        }
        assert_eq!(
            canonical_url("https://bücher.example/a").unwrap().host_str(),
            Some("xn--bcher-kva.example")
        );
        assert_eq!(
            canonical_url("https://[2606:4700:4700::1111]:443/a").unwrap().as_str(),
            "https://[2606:4700:4700::1111]/a"
        );
        for bad in [
            "example.com/a",
            "//example.com/a",
            "https://u:p@example.com/a",
            "https://example.com./a",
            "https://example.com/a/../b",
            "https://example.com/%2e%2e/b",
            "https://example.com/a%2fb",
            "https://example.com/a%5cb",
            "https://example.com/%252f",
            "https://example.com/%zz",
            "https://example.com:0/",
            "https://0x08080808/",
            "https://example.com\\evil/a",
        ] {
            assert!(canonical_url(bad).is_err(), "{bad}");
        }
        assert!(Rule::new("https://example.com/docs/*?q=1", "docs", &[]).is_err());
        let query = canonical_url("https://example.com/search?q=TCP%2fIP%20%2520").unwrap();
        assert_eq!(query.host_str(), Some("example.com"));
        assert_eq!(query.path(), "/search");
        assert_eq!(query.query(), Some("q=TCP%2FIP%20%2520"));
        assert!(canonical_url("https://example.com/search?q=%0A").is_err());
        assert!(Rule::new("https://example.com/*", "default", &[])
            .unwrap()
            .permits(&query));
    }

    #[test]
    fn legacy_rows_become_only_saved_targets_and_invalid_policy_fails_whole() {
        let bytes = br#"{"entries":[{"scheme":"https","host":"example.com","port":"","path":"/docs","raw":"https://www.example.com/"}]}"#;
        let p = Policy::parse(bytes).unwrap();
        assert!(p.authorize("https://example.com/docs", None).is_ok());
        assert!(p.authorize("https://example.com/docs/child", None).is_err());
        assert!(p.authorize("https://www.example.com/", None).is_err());
        assert!(p.authorize("https://example.com/docs", Some("other")).is_err());
        let mut value = serde_json::to_value(&p).unwrap();
        value["rules"][0]["pattern"] = serde_json::json!("https://example.com/docs/*");
        let wildcard = Policy::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(wildcard.rules[0].start_urls().is_empty());
        value["rules"][0]["seeds"] = serde_json::json!(["https://other.test/"]);
        assert!(Policy::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        for bad in [
            br#"{}"#.as_slice(),
            br#"{"version":9,"rules":[]}"#,
            br#"{"entries":[{}]}"#,
            br#"{"version":1,"rules":[],"typo":true}"#,
        ] {
            assert!(Policy::parse(bad).is_err());
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web_allowlist.json");
        assert!(Policy::load(&path).is_err());
        std::fs::write(&path, vec![b' '; MAX_POLICY_BYTES as usize + 1]).unwrap();
        assert!(Policy::load(&path).is_err());
    }
}
