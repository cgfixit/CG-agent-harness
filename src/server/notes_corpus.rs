//! Owner-jailed `.md`/`.txt` notes for local chat. Separate from `memory_notes`.
//! Ranking never confers permission; bytes leave this jail only after owner match.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::common::config::AppConfig;
use crate::common::errors::{HarnessError, Result};
use crate::common::injection::Scanner;
use crate::common::sha256_bytes_hex;
use crate::server::attachments::{blob_section, wrap_fence, IncomingFile, VerifiedAttachment, FENCE_CLOSE, FENCE_OPEN};
use crate::server::passage_index::{chunk_text, retrieve_passages, PassageDoc, SourceKind};

const INDEX_NAME: &str = "index.json";
const BLOB_MODE: u32 = 0o600;
const DEFAULT_MAX_FILES_PER_OWNER: u64 = 64;
const DEFAULT_MAX_FILE_BYTES: u64 = 524_288;
const DEFAULT_MAX_HOME_BYTES: u64 = 8_388_608;
const DEFAULT_MAX_FILES_PER_REQUEST: u64 = 8;
const DEFAULT_MAX_CONCURRENT: u64 = 2;

#[derive(Debug, Clone, Copy)]
pub struct NotesLimits {
    pub max_files_per_owner: usize,
    pub max_file_bytes: u64,
    pub max_home_bytes: u64,
    pub max_files_per_request: usize,
    pub max_concurrent_ingests: usize,
}

impl NotesLimits {
    pub fn from_config(cfg: &AppConfig) -> Result<Self> {
        Ok(Self {
            max_files_per_owner: bounded_u64(
                cfg,
                "notes_corpus.max_files_per_owner",
                DEFAULT_MAX_FILES_PER_OWNER,
                1,
                256,
            )? as usize,
            max_file_bytes: bounded_u64(
                cfg,
                "notes_corpus.max_file_bytes",
                DEFAULT_MAX_FILE_BYTES,
                1024,
                1_048_576,
            )?,
            max_home_bytes: bounded_u64(
                cfg,
                "notes_corpus.max_home_bytes",
                DEFAULT_MAX_HOME_BYTES,
                1_048_576,
                33_554_432,
            )?,
            max_files_per_request: bounded_u64(
                cfg,
                "notes_corpus.max_files_per_request",
                DEFAULT_MAX_FILES_PER_REQUEST,
                1,
                16,
            )? as usize,
            max_concurrent_ingests: bounded_u64(
                cfg,
                "notes_corpus.max_concurrent_ingests",
                DEFAULT_MAX_CONCURRENT,
                1,
                8,
            )? as usize,
        })
    }
}

fn bounded_u64(cfg: &AppConfig, key: &str, default: u64, min: u64, max: u64) -> Result<u64> {
    match cfg.get(key) {
        None => Ok(default),
        Some(v) => {
            let Some(n) = v.as_u64() else {
                return Err(HarnessError::config(format!(
                    "{key} must be an integer from {min} to {max}"
                )));
            };
            if n < min || n > max {
                return Err(HarnessError::config(format!(
                    "{key} must be an integer from {min} to {max}"
                )));
            }
            Ok(n)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteBlob {
    pub id: String,
    pub owner: String,
    pub sha256: String,
    pub magic_mime: String,
    pub byte_len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Manifest {
    #[serde(default)]
    notes: Vec<NoteBlob>,
}

#[derive(Debug)]
pub struct NotesCorpus {
    jail: Dir,
    lock: Mutex<()>,
    limits: NotesLimits,
}

impl NotesCorpus {
    pub fn open(root: &Path, limits: NotesLimits) -> Result<Self> {
        if std::fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(HarnessError::new(
                "IO_ERROR",
                "notes_corpus directory must not be a symlink",
            ));
        }
        std::fs::create_dir_all(root)
            .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot create notes_corpus directory: {e}")))?;
        let (parent, name) = match (root.parent(), root.file_name()) {
            (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => (parent, name),
            _ => {
                return Err(HarnessError::new(
                    "IO_ERROR",
                    "notes_corpus directory must have a parent directory",
                ))
            }
        };
        let parent_dir = Dir::open_ambient_dir(parent, cap_std::ambient_authority())
            .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot open notes_corpus parent: {e}")))?;
        let jail = parent_dir
            .open_dir(name)
            .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot open notes_corpus directory: {e}")))?;
        if std::fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(HarnessError::new(
                "IO_ERROR",
                "notes_corpus directory must not be a symlink",
            ));
        }
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt;
            use std::os::unix::fs::MetadataExt as StdMetadataExt;
            let path_meta = std::fs::symlink_metadata(root)
                .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot stat notes_corpus directory: {e}")))?;
            let dir_meta = jail
                .dir_metadata()
                .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot stat notes_corpus directory: {e}")))?;
            if path_meta.file_type().is_symlink()
                || StdMetadataExt::dev(&path_meta) != dir_meta.dev()
                || StdMetadataExt::ino(&path_meta) != dir_meta.ino()
            {
                return Err(HarnessError::new(
                    "IO_ERROR",
                    "notes_corpus directory must not be a symlink",
                ));
            }
        }
        Ok(Self {
            jail,
            lock: Mutex::new(()),
            limits,
        })
    }

    pub fn limits(&self) -> NotesLimits {
        self.limits
    }

    pub fn store(&self, owner: &str, files: &[IncomingFile]) -> Result<Vec<NoteBlob>> {
        if files.is_empty() {
            return Err(HarnessError::new("NOTES_EMPTY", "no notes files were provided"));
        }
        if files.len() > self.limits.max_files_per_request {
            return Err(
                HarnessError::new("NOTES_TOO_MANY", "too many notes files in this request")
                    .detail("limit", self.limits.max_files_per_request as u64),
            );
        }
        let classified: Vec<(IncomingFile, &'static str, String)> = files
            .iter()
            .map(|f| {
                let ext = allowed_ext(&f.filename)?;
                let text = classify_note(&f.data, ext, self.limits.max_file_bytes)?;
                Ok((
                    IncomingFile {
                        filename: String::new(),
                        data: f.data.clone(),
                    },
                    mime_for(ext),
                    text,
                ))
            })
            .collect::<Result<_>>()?;
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut manifest = self.load_manifest()?;
        let owner_count = manifest.notes.iter().filter(|n| n.owner == owner).count();
        if owner_count + classified.len() > self.limits.max_files_per_owner {
            return Err(HarnessError::new("NOTES_QUOTA", "owner notes file limit reached")
                .detail("limit", self.limits.max_files_per_owner as u64));
        }
        let used: u64 = manifest.notes.iter().map(|n| n.byte_len).sum();
        let incoming: u64 = classified.iter().map(|(f, _, _)| f.data.len() as u64).sum();
        if used.saturating_add(incoming) > self.limits.max_home_bytes {
            return Err(HarnessError::new("NOTES_QUOTA", "notes corpus byte quota reached")
                .detail("limit_bytes", self.limits.max_home_bytes));
        }
        let mut stored = Vec::new();
        let mut written = Vec::new();
        for (file, mime, _text) in classified {
            let id = Uuid::new_v4().hyphenated().to_string();
            let blob = NoteBlob {
                id: id.clone(),
                owner: owner.to_string(),
                sha256: sha256_bytes_hex(&file.data),
                magic_mime: mime.to_string(),
                byte_len: file.data.len() as u64,
            };
            let rel = note_rel(&blob)?;
            if let Some(parent) = rel.parent() {
                self.jail
                    .create_dir_all(parent)
                    .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot create owner notes directory: {e}")))?;
            }
            if let Err(e) = write_in_jail(&self.jail, &rel, &file.data) {
                unlink_all(&self.jail, &written);
                return Err(e);
            }
            written.push(rel);
            stored.push(blob);
        }
        manifest.notes.extend(stored.iter().cloned());
        if let Err(e) = self.save_manifest(&manifest) {
            unlink_all(&self.jail, &written);
            return Err(e);
        }
        Ok(stored)
    }

    pub fn list_for_owner(&self, owner: &str) -> Result<Vec<NoteBlob>> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let manifest = self.load_manifest()?;
        Ok(manifest.notes.into_iter().filter(|n| n.owner == owner).collect())
    }

    pub fn unlink_id(&self, owner: &str, id: &str) -> Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut manifest = self.load_manifest()?;
        let Some(idx) = manifest.notes.iter().position(|n| n.owner == owner && n.id == id) else {
            return Err(HarnessError::new("NOTES_NOT_FOUND", "note is not readable"));
        };
        let blob = manifest.notes.remove(idx);
        let rel = note_rel(&blob)?;
        let _ = self.jail.remove_file(&rel);
        self.save_manifest(&manifest)?;
        Ok(())
    }

    pub fn unlink_owner(&self, owner: &str) -> Result<usize> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut manifest = self.load_manifest()?;
        let mut n = 0usize;
        let mut keep = Vec::new();
        for blob in manifest.notes.drain(..) {
            if blob.owner == owner {
                if let Ok(rel) = note_rel(&blob) {
                    let _ = self.jail.remove_file(&rel);
                }
                n += 1;
            } else {
                keep.push(blob);
            }
        }
        manifest.notes = keep;
        self.save_manifest(&manifest)?;
        Ok(n)
    }

    pub fn verified_texts(&self, owner: &str) -> Result<Vec<VerifiedAttachment>> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let manifest = self.load_manifest()?;
        let scanner = Scanner::core();
        let mut out = Vec::new();
        for blob in manifest.notes.iter().filter(|n| n.owner == owner) {
            let rel = note_rel(blob)?;
            let bytes = self
                .jail
                .read(&rel)
                .map_err(|_| HarnessError::new("NOTES_NOT_FOUND", "note is not readable"))?;
            if bytes.len() as u64 != blob.byte_len || sha256_bytes_hex(&bytes) != blob.sha256 {
                return Err(HarnessError::new("NOTES_NOT_FOUND", "note is not readable"));
            }
            let ext = if blob.magic_mime == "text/markdown" {
                "md"
            } else {
                "txt"
            };
            let text = classify_note(&bytes, ext, self.limits.max_file_bytes)?;
            let injection_hits = scanner.scan(&text).len();
            let (text, sentinels_removed) = neutralize(&text);
            out.push(VerifiedAttachment {
                blob: crate::server::attachments::AttachmentBlob {
                    id: blob.id.clone(),
                    owner: blob.owner.clone(),
                    sha256: blob.sha256.clone(),
                    magic_mime: blob.magic_mime.clone(),
                    byte_len: blob.byte_len,
                },
                text,
                sentinels_removed,
                injection_hits,
            });
        }
        Ok(out)
    }

    pub fn fence_for_query(&self, owner: &str, query: &str, budget: usize) -> Result<String> {
        if budget == 0 {
            return Ok(String::new());
        }
        let verified = self.verified_texts(owner)?;
        if verified.is_empty() {
            return Ok(String::new());
        }
        if query.trim().is_empty() {
            return Ok(notes_fence(&verified, budget));
        }
        match notes_passage_fence(&verified, query, budget) {
            Ok(Some(fence)) => Ok(fence),
            Ok(None) | Err(_) => Ok(notes_fence(&verified, budget)),
        }
    }

    fn load_manifest(&self) -> Result<Manifest> {
        match self.jail.read(INDEX_NAME) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| HarnessError::new("IO_ERROR", "notes_corpus index is not valid JSON")),
            Err(_) => Ok(Manifest::default()),
        }
    }

    fn save_manifest(&self, manifest: &Manifest) -> Result<()> {
        let payload = serde_json::to_vec_pretty(manifest)
            .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot serialize notes index: {e}")))?;
        write_in_jail(&self.jail, Path::new(INDEX_NAME), &payload)
    }
}

fn notes_fence(verified: &[VerifiedAttachment], budget: usize) -> String {
    let mut sections = Vec::new();
    let mut remaining = budget;
    for item in verified {
        sections.push(note_section(item, item.text.as_str(), remaining));
        let shown = crate::common::clip_chars(&item.text, remaining).chars().count();
        remaining = remaining.saturating_sub(shown);
    }
    wrap_notes(&sections)
}

fn notes_passage_fence(verified: &[VerifiedAttachment], query: &str, budget: usize) -> Result<Option<String>> {
    let mut docs = Vec::new();
    for item in verified {
        for (start, end, heading, text) in chunk_text(&item.text) {
            docs.push(PassageDoc {
                source: SourceKind::NotesCorpus,
                source_id: item.blob.id.clone(),
                heading,
                text,
                sha256: item.blob.sha256.clone(),
                start,
                end,
                score: 0.0,
            });
        }
    }
    let hits = retrieve_passages(&docs, query, 16)?;
    if hits.is_empty() {
        return Ok(None);
    }
    let mut by_id: std::collections::BTreeMap<&str, Vec<&PassageDoc>> = std::collections::BTreeMap::new();
    for hit in &hits {
        by_id.entry(hit.source_id.as_str()).or_default().push(hit);
    }
    let mut sections = Vec::new();
    let mut remaining = budget;
    for item in verified {
        if let Some(passages) = by_id.get(item.blob.id.as_str()) {
            let mut body = String::new();
            for (i, p) in passages.iter().enumerate() {
                if i > 0 {
                    body.push_str("\n\n");
                }
                body.push_str(&p.text);
            }
            sections.push(note_section(item, &body, remaining));
            remaining = remaining.saturating_sub(crate::common::clip_chars(&body, remaining).chars().count());
        }
    }
    Ok(Some(wrap_notes(&sections)))
}

fn note_section(item: &VerifiedAttachment, shown: &str, remaining: usize) -> String {
    blob_section(item, shown, remaining)
        .replace("source=attachment", "source=notes_corpus")
        .replace("### attachment ", "### note ")
}

fn wrap_notes(sections: &[String]) -> String {
    if sections.is_empty() {
        return String::new();
    }
    let inner = wrap_fence(sections);
    inner.replace(
        "Operator file attachments (read-only)",
        "Operator notes corpus (read-only)",
    )
}

fn neutralize(text: &str) -> (String, usize) {
    let mut count = 0usize;
    let mut out = text.to_string();
    for marker in [FENCE_CLOSE, FENCE_OPEN] {
        let hits = out.matches(marker).count();
        if hits > 0 {
            count += hits;
            out = out.replace(marker, "[reserved fence marker removed]");
        }
    }
    (out, count)
}

fn allowed_ext(filename: &str) -> Result<&'static str> {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or(filename).trim();
    if base.is_empty() || base.contains("..") {
        return Err(HarnessError::new("NOTES_TYPE", "filename is not an allowed notes type"));
    }
    match base.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
        Some("md") => Ok("md"),
        Some("txt") => Ok("txt"),
        _ => Err(HarnessError::new("NOTES_TYPE", "only .md and .txt notes are accepted")),
    }
}

fn mime_for(ext: &str) -> &'static str {
    if ext == "md" {
        "text/markdown"
    } else {
        "text/plain"
    }
}

fn classify_note(data: &[u8], ext: &str, max_bytes: u64) -> Result<String> {
    if data.is_empty() {
        return Err(HarnessError::new("NOTES_EMPTY", "file is empty"));
    }
    if data.len() as u64 > max_bytes {
        return Err(
            HarnessError::new("NOTES_TOO_LARGE", "file exceeds the notes size limit").detail("limit_bytes", max_bytes),
        );
    }
    if data.contains(&0) {
        return Err(HarnessError::new("NOTES_NUL", "NUL bytes are not allowed in notes"));
    }
    let text = std::str::from_utf8(data)
        .map_err(|_| HarnessError::new("NOTES_ENCODING", "file is not valid UTF-8 text"))?
        .to_string();
    let _ = ext;
    Ok(text)
}

fn note_rel(blob: &NoteBlob) -> Result<PathBuf> {
    let id =
        Uuid::parse_str(blob.id.trim()).map_err(|_| HarnessError::new("NOTES_NOT_FOUND", "note is not readable"))?;
    let owner_key = crate::common::sha256_hex(&blob.owner);
    if owner_key.len() != 64 || !owner_key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(HarnessError::new("NOTES_NOT_FOUND", "note is not readable"));
    }
    Ok(PathBuf::from(owner_key).join(id.hyphenated().to_string()))
}

fn write_in_jail(jail: &Dir, rel: &Path, data: &[u8]) -> Result<()> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(BLOB_MODE);
    }
    let mut file = jail
        .open_with(rel, &options)
        .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot write notes blob: {e}")))?;
    file.write_all(data)
        .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot write notes blob: {e}")))?;
    file.sync_all()
        .map_err(|e| HarnessError::new("IO_ERROR", format!("cannot sync notes blob: {e}")))?;
    Ok(())
}

fn unlink_all(jail: &Dir, rels: &[PathBuf]) {
    for rel in rels {
        let _ = jail.remove_file(rel);
    }
}

pub fn list_json(notes: &[NoteBlob]) -> Value {
    json!({
        "count": notes.len(),
        "notes": notes.iter().map(|n| json!({
            "id": n.id,
            "magic_mime": n.magic_mime,
            "sha256_prefix": n.sha256.chars().take(12).collect::<String>(),
            "byte_len": n.byte_len,
        })).collect::<Vec<_>>(),
    })
}

pub fn ingest_error(err: &HarnessError) -> crate::server::errors::ApiError {
    let status = match err.code.as_str() {
        "NOTES_TOO_LARGE" | "NOTES_QUOTA" => axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        "NOTES_BUSY" => axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "NOTES_NOT_FOUND" => axum::http::StatusCode::NOT_FOUND,
        "IO_ERROR" => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "ATTACHMENT_SURFACE_FORBIDDEN" => axum::http::StatusCode::BAD_REQUEST,
        _ => axum::http::StatusCode::BAD_REQUEST,
    };
    crate::server::errors::ApiError::from_err(status, err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::prompts::MAX_WEB_CHARS;

    fn limits() -> NotesLimits {
        NotesLimits {
            max_files_per_owner: 64,
            max_file_bytes: 524_288,
            max_home_bytes: 8_388_608,
            max_files_per_request: 8,
            max_concurrent_ingests: 2,
        }
    }

    fn file(name: &str, body: &str) -> IncomingFile {
        IncomingFile {
            filename: name.into(),
            data: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn owner_isolation_and_md_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = NotesCorpus::open(&dir.path().join("notes"), limits()).unwrap();
        let alice = store.store("alice", &[file("a.md", "alice-secret-token")]).unwrap();
        assert!(store.store("alice", &[file("b.json", "{}")]).is_err());
        let fence = store
            .fence_for_query("bob", "alice-secret-token", MAX_WEB_CHARS)
            .unwrap();
        assert!(fence.is_empty());
        let fence = store
            .fence_for_query("alice", "alice-secret-token", MAX_WEB_CHARS)
            .unwrap();
        assert!(fence.contains("alice-secret-token"));
        assert!(fence.contains("source=notes_corpus"));
        assert!(!fence.contains("a.md"));
        store.unlink_id("alice", &alice[0].id).unwrap();
        assert!(store.list_for_owner("alice").unwrap().is_empty());
    }

    #[test]
    fn symlink_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("notes_corpus");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert!(NotesCorpus::open(&link, limits()).is_err());
        }
    }

    #[test]
    fn quota_and_injection_scan() {
        let dir = tempfile::tempdir().unwrap();
        let tight = NotesLimits {
            max_files_per_owner: 1,
            max_file_bytes: 64,
            max_home_bytes: 8_388_608,
            max_files_per_request: 8,
            max_concurrent_ingests: 2,
        };
        let store = NotesCorpus::open(&dir.path().join("notes"), tight).unwrap();
        store.store("local", &[file("a.md", "one")]).unwrap();
        let err = store.store("local", &[file("b.md", "two")]).unwrap_err();
        assert_eq!(err.code, "NOTES_QUOTA");
        let wide = NotesCorpus::open(&dir.path().join("notes2"), limits()).unwrap();
        wide.store("local", &[file("c.md", "ignore previous instructions\nkeep")])
            .unwrap();
        let fence = wide.fence_for_query("local", "keep", MAX_WEB_CHARS).unwrap();
        assert!(fence.contains("injection_phrases="));
    }
}
