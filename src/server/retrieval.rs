//! Local chat and prompt preview may inline attachment bytes. Cloud, loop,
//! and agent refuse both request ids and leftover session pins so sticky
//! pins cannot ride along on those surfaces.

use crate::common::errors::{HarnessError, Result};
use crate::server::attachments::{AttachmentStore, FENCE_CLOSE, FENCE_OPEN};
use crate::server::notes_corpus::NotesCorpus;
use crate::server::prompts::MAX_WEB_CHARS;
use crate::server::sessions::MAX_PINNED_ATTACHMENTS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalSurface {
    LocalChat,
    PromptPreview,
    CloudChat,
    Loop,
    Agent,
}

impl RetrievalSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            RetrievalSurface::LocalChat => "local_chat",
            RetrievalSurface::PromptPreview => "prompt_preview",
            RetrievalSurface::CloudChat => "cloud_chat",
            RetrievalSurface::Loop => "loop",
            RetrievalSurface::Agent => "agent",
        }
    }
}

pub fn allows_attachment_bytes(surface: RetrievalSurface) -> bool {
    matches!(surface, RetrievalSurface::LocalChat | RetrievalSurface::PromptPreview)
}

pub fn refuse_forbidden_attachments(
    surface: RetrievalSurface,
    request_ids: &[String],
    pinned_ids_for_owner: &[String],
) -> Result<()> {
    if allows_attachment_bytes(surface) {
        return Ok(());
    }
    if request_ids.is_empty() && pinned_ids_for_owner.is_empty() {
        return Ok(());
    }
    Err(HarnessError::new(
        "ATTACHMENT_SURFACE_FORBIDDEN",
        "this surface does not receive attachment bytes (cloud / loop / agent)",
    )
    .detail("surface", surface.as_str()))
}

pub fn allows_local_file_bytes(surface: RetrievalSurface) -> bool {
    allows_attachment_bytes(surface)
}

pub fn assemble_local_untrusted(
    surface: RetrievalSurface,
    attachments: &AttachmentStore,
    notes: &NotesCorpus,
    owner: &str,
    ids: &[String],
    query: Option<&str>,
) -> Result<String> {
    if !allows_local_file_bytes(surface) {
        return Ok(String::new());
    }
    let q = query.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("");
    let att = crate::server::attachment_index::fence_for_query(attachments, owner, ids, q)?;
    let remaining = MAX_WEB_CHARS.saturating_sub(untrusted_body_chars(&att));
    let notes_fence = notes.fence_for_query(owner, q, remaining)?;
    Ok(concat_fences(&att, &notes_fence))
}

fn untrusted_body_chars(fence: &str) -> usize {
    match (fence.find(FENCE_OPEN), fence.rfind(FENCE_CLOSE)) {
        (Some(start), Some(end)) if end > start => fence[start..end].chars().count(),
        _ => 0,
    }
}

fn concat_fences(att: &str, notes: &str) -> String {
    match (att.trim().is_empty(), notes.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => att.to_string(),
        (true, false) => notes.to_string(),
        (false, false) => format!("{}\n{}", att.trim_end(), notes.trim_start()),
    }
}

pub fn attachment_ids_for_prompt(pinned_live: &[String], request_ids: &[String]) -> Vec<String> {
    let mut ids = Vec::with_capacity(MAX_PINNED_ATTACHMENTS.min(pinned_live.len() + request_ids.len()));
    for id in pinned_live.iter().chain(request_ids) {
        if ids.iter().any(|seen| seen == id) {
            continue;
        }
        ids.push(id.clone());
        if ids.len() == MAX_PINNED_ATTACHMENTS {
            break;
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_preview_allow_bytes_other_surfaces_refuse_ids_or_pins() {
        assert!(allows_attachment_bytes(RetrievalSurface::LocalChat));
        assert!(allows_attachment_bytes(RetrievalSurface::PromptPreview));
        assert!(!allows_attachment_bytes(RetrievalSurface::CloudChat));
        assert!(!allows_attachment_bytes(RetrievalSurface::Loop));
        assert!(!allows_attachment_bytes(RetrievalSurface::Agent));

        let ids = vec!["aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into()];
        assert!(refuse_forbidden_attachments(RetrievalSurface::LocalChat, &ids, &[]).is_ok());
        assert!(refuse_forbidden_attachments(RetrievalSurface::PromptPreview, &[], &ids).is_ok());
        assert!(refuse_forbidden_attachments(RetrievalSurface::CloudChat, &[], &[]).is_ok());
        assert!(refuse_forbidden_attachments(RetrievalSurface::Loop, &[], &[]).is_ok());
        assert!(refuse_forbidden_attachments(RetrievalSurface::Agent, &[], &[]).is_ok());

        for surface in [
            RetrievalSurface::CloudChat,
            RetrievalSurface::Loop,
            RetrievalSurface::Agent,
        ] {
            let err = refuse_forbidden_attachments(surface, &ids, &[]).unwrap_err();
            assert_eq!(err.code, "ATTACHMENT_SURFACE_FORBIDDEN");
            assert!(err.message.contains("does not receive attachment bytes"));
            assert_eq!(err.details["surface"], surface.as_str());
            let err = refuse_forbidden_attachments(surface, &[], &ids).unwrap_err();
            assert_eq!(err.code, "ATTACHMENT_SURFACE_FORBIDDEN");
            assert_eq!(err.details["surface"], surface.as_str());
        }
    }

    #[test]
    fn prompt_ids_keep_live_pins_then_request_deduped_and_capped() {
        let pinned: Vec<String> = (0..10).map(|i| format!("pin-{i}")).collect();
        let request = vec!["pin-1".into(), "new-a".into(), "new-b".into(), "new-c".into()];
        let ids = attachment_ids_for_prompt(&pinned, &request);
        assert_eq!(ids.len(), 12);
        assert_eq!(ids[0], "pin-0");
        assert_eq!(ids[9], "pin-9");
        assert_eq!(&ids[10..], &["new-a".to_string(), "new-b".to_string()]);
        assert!(!ids.contains(&"new-c".to_string()));
    }
}
