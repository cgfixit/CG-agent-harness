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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportHeader {
    pub session_id: String,
    pub title: String,
    pub created_ts: f64,
    pub model: String,
    pub goal: String,
}

pub fn render(session: &Session) -> String {
    let header = ExportHeader {
        session_id: session.session_id.clone(),
        title: session.title.clone(),
        created_ts: session.created_ts,
        model: session.model.clone(),
        goal: session.goal.clone(),
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

pub fn write_export(home: &Home, session: &Session) -> Result<PathBuf> {
    let dir = home.exports_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.md", session.session_id));
    let bytes = render(session).into_bytes();
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
            goal: "do the thing".into(),
            selected_skills: Vec::new(),
            selected_facts: Vec::new(),
            last_prompt_skills: Vec::new(),
            goal_stage: None,
        }
    }

    #[test]
    fn round_trips_fences_headers_and_compaction_marker() {
        let s = session("```\n### user\nnot a header\n```\n<!--/cgagentharness-msg-->");
        let md = render(&s);
        let (header, messages) = parse(&md).unwrap();
        assert_eq!(header.session_id, "aaaaaaaaaaaa");
        assert_eq!(header.goal, "do the thing");
        assert_eq!(messages[0].text, s.messages[0].text);
        assert_eq!(messages[1].text, "[session-compacted]\nkept");
    }
}
