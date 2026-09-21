//! Markdown export of a chat session. Local files are 0o600; the HTTP body is
//! the same bytes. Round-trip parse does not invent Message fields.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::common::atomic::write_atomic;
use crate::common::errors::{HarnessError, Result};
use crate::common::home::Home;
use crate::server::sessions::{Message, Session, SESSION_ERROR_CODE};

const META_OPEN: &str = "<!--cgagentharness-meta ";
const META_CLOSE: &str = " -->";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportAttachmentPin {
    pub id: String,
    pub magic_mime: String,
    pub sha256_prefix: String,
    pub omitted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportHeader {
    pub session_id: String,
    pub title: String,
    pub created_ts: f64,
    pub model: String,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_pins: Vec<ExportAttachmentPin>,
}

pub fn render(session: &Session, attachment_pins: Vec<ExportAttachmentPin>) -> String {
    let header = ExportHeader {
        session_id: session.session_id.clone(),
        title: session.title.clone(),
        created_ts: session.created_ts,
        model: session.model.clone(),
        goal: session.goal.clone(),
        attachment_pins,
    };
    let meta = serde_json::to_string(&header).unwrap_or_else(|_| "{}".into());
    let mut out = String::new();
    out.push_str(META_OPEN);
    out.push_str(&meta);
    out.push_str(META_CLOSE);
    out.push('\n');
    for msg in &session.messages {
        let mut boundary = "cgagentharness-msg".to_string();
        let closer = loop {
            let c = format!("<!--/{boundary}-->");
            if !msg.text.contains(&c) {
                break c;
            }
            boundary.push('x');
        };
        out.push('\n');
        out.push_str(&format!(
            "<!--{boundary} role=\"{}\" ts=\"{}\"-->\n",
            json!(msg.role).as_str().unwrap_or("user"),
            msg.ts
        ));
        out.push_str(&msg.text);
        if !msg.text.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&closer);
        out.push('\n');
    }
    out
}

pub fn parse(md: &str) -> Result<(ExportHeader, Vec<Message>)> {
    let Some(rest) = md.strip_prefix(META_OPEN) else {
        return Err(session_err("missing export header"));
    };
    let Some(end) = rest.find(META_CLOSE) else {
        return Err(session_err("truncated export header"));
    };
    let header: ExportHeader =
        serde_json::from_str(&rest[..end]).map_err(|_| session_err("unreadable export header"))?;
    let mut body = &rest[end + META_CLOSE.len()..];
    let mut messages = Vec::new();
    while let Some(open_at) = body.find("<!--cgagentharness-msg") {
        body = &body[open_at + 4..];
        let Some(tag_end) = body.find("-->") else {
            return Err(session_err("truncated message tag"));
        };
        let tag = &body[..tag_end];
        let boundary = tag.split_whitespace().next().unwrap_or("cgagentharness-msg");
        let role = attr(tag, "role").unwrap_or("user");
        let ts = attr(tag, "ts").and_then(|s| s.parse().ok()).unwrap_or(0.0);
        body = &body[tag_end + 3..];
        if body.starts_with('\n') {
            body = &body[1..];
        }
        let closer = format!("<!--/{boundary}-->");
        let Some(close_at) = body.find(&closer) else {
            return Err(session_err("unterminated message"));
        };
        let mut text = body[..close_at].to_string();
        if text.ends_with('\n') {
            text.pop();
        }
        messages.push(Message {
            role: role.to_string(),
            text,
            ts,
        });
        body = &body[close_at + closer.len()..];
    }
    Ok((header, messages))
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let start = tag.find(&key)? + key.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn session_err(message: &str) -> HarnessError {
    HarnessError::new(SESSION_ERROR_CODE, message)
}

/// Twelve lowercase hex digits plus `.md`, built only from an integer.
fn export_filename(n: u64) -> Result<PathBuf> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut name = [0u8; 15];
    let mut x = n;
    for i in (0..12).rev() {
        name[i] = HEX[(x & 0xf) as usize];
        x >>= 4;
    }
    name[12] = b'.';
    name[13] = b'm';
    name[14] = b'd';
    let s = std::str::from_utf8(&name).map_err(|_| session_err("export name"))?;
    Ok(PathBuf::from(s))
}

pub fn write_export(home: &Home, session: &Session, attachment_pins: Vec<ExportAttachmentPin>) -> Result<PathBuf> {
    let n = crate::server::sessions::session_id_u64(&session.session_id)?;
    let dir = home.exports_dir();
    let path = dir.join(export_filename(n)?);
    let bytes = render(session, attachment_pins).into_bytes();
    write_atomic(&path, &bytes, Some(0o600))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::sessions::TokenTally;

    fn session(text: &str) -> Session {
        Session {
            session_id: "aaaaaaaaaaaa".into(),
            title: "t".into(),
            created_ts: 1.5,
            model: "qwen".into(),
            messages: vec![
                Message {
                    role: "user".into(),
                    text: text.into(),
                    ts: 1.0,
                },
                Message {
                    role: "assistant".into(),
                    text: "[session-compacted]\nkept".into(),
                    ts: 2.0,
                },
            ],
            prompt_history: Vec::new(),
            tally: TokenTally::default(),
            token_calibration: None,
            goal: "do the thing".into(),
            selected_skills: Vec::new(),
            style: None,
            selected_facts: Vec::new(),
            last_prompt_skills: Vec::new(),
            goal_stage: None,
            attachment_pins: Vec::new(),
        }
    }

    #[test]
    fn write_export_rejects_a_path_shaped_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let home = crate::common::home::Home::at(tmp.path().to_path_buf());
        home.ensure_layout().unwrap();
        let mut s = session("hi");
        s.session_id = "../etc/passwd".into();
        assert!(write_export(&home, &s, Vec::new()).is_err());
        assert!(!home.exports_dir().join("../etc/passwd.md").exists());
        assert!(!home.exports_dir().join("aaaaaaaaaaaa.md").exists());
        let ok = session("hi");
        let written = write_export(&home, &ok, Vec::new()).unwrap();
        assert_eq!(written.file_name().unwrap(), "aaaaaaaaaaaa.md");
        assert!(written.is_file());
    }

    #[test]
    fn round_trips_fences_headers_and_compaction_marker() {
        let s = session("```\n### user\nnot a header\n```\n<!--/cgagentharness-msg-->");
        let md = render(&s, Vec::new());
        let (header, messages) = parse(&md).unwrap();
        assert_eq!(header.session_id, "aaaaaaaaaaaa");
        assert_eq!(header.goal, "do the thing");
        assert!(header.attachment_pins.is_empty());
        assert_eq!(messages[0].text, s.messages[0].text);
        assert_eq!(messages[1].text, "[session-compacted]\nkept");
    }

    #[test]
    fn export_header_pins_omit_filename_and_body() {
        let pins = vec![
            ExportAttachmentPin {
                id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
                magic_mime: "text/plain".into(),
                sha256_prefix: "0123456789ab".into(),
                omitted: false,
            },
            ExportAttachmentPin {
                id: "bbbbbbbb-cccc-dddd-eeee-ffffffffffff".into(),
                magic_mime: String::new(),
                sha256_prefix: String::new(),
                omitted: true,
            },
        ];
        let md = render(&session("secret-body"), pins.clone());
        assert!(!md.contains("secret filename"));
        let (header, _) = parse(&md).unwrap();
        assert_eq!(header.attachment_pins, pins);
    }
}
