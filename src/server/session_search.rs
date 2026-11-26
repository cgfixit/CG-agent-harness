//! Per-owner in-memory Tantivy index over session transcripts. Not the web cache.
//!
//! Each search reconciles the caller's index to that owner's session files
//! before the query. A missing index, a schema mismatch, or a doc-count
//! mismatch is rebuilt from those files. The index is never written to disk.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TantivyDocument, Term, TextFieldIndexing, TextOptions, Value as _, STORED,
};
use tantivy::{doc, Index, IndexReader};

use crate::common::errors::{HarnessError, Result};
use crate::server::sessions::{OwnedSessionStore, Session};

const MAX_SESSIONS: usize = 200;
const MAX_BYTES: usize = 2_000_000;
const CHUNK_CHARS: usize = 1200;
const MAX_QUERY_CHARS: usize = 200;
const MAX_HITS: usize = 16;
const SNIPPET_CHARS: usize = 160;
const PER_SESSION: usize = 2;
const SCHEMA_VERSION: u32 = 1;
const WRITER_HEAP: usize = 32_000_000;
/// `char::to_lowercase` yields at most 3 scalars. A query of `MAX_QUERY_CHARS`
/// can therefore need this much overlap to keep a phrase inside one chunk.
/// Hit text is trimmed back to the query's own overlap before the literal
/// check, so snippets stay on the same window as a per-query index.
const INDEX_OVERLAP: usize = MAX_QUERY_CHARS * 3 - 1;

#[derive(Debug, Clone)]
pub struct SessionHit {
    pub session_id: String,
    pub title: String,
    pub snippet: String,
    pub role: String,
    pub ts: f64,
    pub score: f32,
}

pub fn search_sessions(store: &OwnedSessionStore<'_>, query: &str) -> Result<Vec<SessionHit>> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > MAX_QUERY_CHARS {
        return Ok(Vec::new());
    }
    let Some(expression) = super::retrieval_ranking::tantivy_expression(query) else {
        return Ok(Vec::new());
    };
    let lower_query = query.to_lowercase();
    let overlap = lower_query.chars().count() - 1;

    #[cfg(test)]
    let prepared_at = std::time::Instant::now();
    let slot = slot_for(store.storage_dir(), store.owner_id());
    // One index writer per owner. A second search for the same owner waits.
    // Other owners use other slots, so they do not share this lock.
    let mut guard = slot.inner.lock().unwrap_or_else(|poison| poison.into_inner());
    let (stats, rebuilt) = ensure_index(&mut guard, store)?;
    #[cfg(test)]
    let prepare_ns = prepared_at.elapsed().as_nanos();
    #[cfg(test)]
    let query_started = std::time::Instant::now();
    let hits = run_query(
        guard.as_ref().expect("index"),
        &expression,
        query,
        &lower_query,
        overlap,
    )?;
    #[cfg(test)]
    TRACE.with(|trace| {
        trace.set(Trace {
            rebuild: rebuilt,
            reread: stats.reread,
            docs: stats.docs,
            prepare_ns,
            query_ns: query_started.elapsed().as_nanos(),
        });
    });
    #[cfg(not(test))]
    let _ = (stats.reread, stats.docs, rebuilt);
    Ok(hits)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
struct Trace {
    rebuild: bool,
    reread: u32,
    docs: u32,
    prepare_ns: u128,
    query_ns: u128,
}

#[cfg(test)]
thread_local! {
    static TRACE: std::cell::Cell<Trace> = const {
        std::cell::Cell::new(Trace {
            rebuild: false,
            reread: 0,
            docs: 0,
            prepare_ns: 0,
            query_ns: 0,
        })
    };
}

#[cfg(test)]
fn testing_trace() -> Trace {
    TRACE.with(|trace| trace.get())
}

struct DeltaStats {
    reread: u32,
    docs: u32,
}

struct Fields {
    title: Field,
    body: Field,
    sid: Field,
    role: Field,
    ts: Field,
}

struct OwnerIndex {
    schema_version: u32,
    index: Index,
    reader: IndexReader,
    fields: Fields,
    sessions: Vec<IndexedSession>,
    expected_docs: u64,
}

#[derive(Clone)]
struct IndexedSession {
    id: String,
    mtime: SystemTime,
    len: u64,
    counted_bytes: usize,
    bytes_before: usize,
    /// True when the byte cap stopped this session before its last message.
    /// A later shift of `bytes_before` then changes which messages are indexed.
    truncated: bool,
}

struct StoredChunk {
    text: String,
    role: String,
    ts: f64,
}

struct WriteSession {
    id: String,
    title: String,
    docs: Vec<StoredChunk>,
}

struct Plan {
    sessions: Vec<IndexedSession>,
    writes: Vec<WriteSession>,
    remove: Vec<String>,
}

struct Slot {
    inner: Mutex<Option<OwnerIndex>>,
}

type SessionIndexCache = HashMap<(PathBuf, String), Arc<Slot>>;

fn cache() -> &'static Mutex<SessionIndexCache> {
    static CACHE: LazyLock<Mutex<SessionIndexCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    &CACHE
}

fn slot_for(dir: &Path, owner: &str) -> Arc<Slot> {
    let mut map = cache().lock().unwrap_or_else(|poison| poison.into_inner());
    map.retain(|key, _| key.0.exists());
    map.entry((dir.to_path_buf(), owner.to_string()))
        .or_insert_with(|| {
            Arc::new(Slot {
                inner: Mutex::new(None),
            })
        })
        .clone()
}

fn failed(message: &'static str) -> HarnessError {
    HarnessError::new("SESSION_SEARCH_FAILED", message)
}

fn healthy(index: &OwnerIndex) -> bool {
    index.schema_version == SCHEMA_VERSION && index.reader.searcher().num_docs() == index.expected_docs
}

fn ensure_index(slot: &mut Option<OwnerIndex>, store: &OwnedSessionStore<'_>) -> Result<(DeltaStats, bool)> {
    let mut rebuilt = false;
    if !slot.as_ref().is_some_and(healthy) {
        *slot = Some(OwnerIndex::empty()?);
        rebuilt = true;
    }
    match apply_delta(slot.as_mut().expect("index"), store) {
        Ok(stats) => Ok((stats, rebuilt)),
        Err(_) => {
            *slot = Some(OwnerIndex::empty()?);
            apply_delta(slot.as_mut().expect("index"), store).map(|stats| (stats, true))
        }
    }
}

impl OwnerIndex {
    fn empty() -> Result<Self> {
        let mut builder = Schema::builder();
        let indexed = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("en_stem")
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );
        let stored_text = indexed.clone().set_stored();
        // `raw` keeps the session id as one term so `delete_term` removes that session only.
        let raw_id = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer("raw")
                    .set_index_option(IndexRecordOption::Basic),
            )
            .set_stored();
        let stored = TextOptions::default().set_stored();
        let fields = Fields {
            title: builder.add_text_field("title", stored_text.clone()),
            body: builder.add_text_field("text", stored_text),
            sid: builder.add_text_field("session_id", raw_id),
            role: builder.add_text_field("role", stored),
            ts: builder.add_f64_field("ts", STORED),
        };
        let index = Index::create_in_ram(builder.build());
        let reader = index.reader().map_err(|_| failed("could not open session index"))?;
        Ok(Self {
            schema_version: SCHEMA_VERSION,
            index,
            reader,
            fields,
            sessions: Vec::new(),
            expected_docs: 0,
        })
    }
}

fn apply_delta(index: &mut OwnerIndex, store: &OwnedSessionStore<'_>) -> Result<DeltaStats> {
    let previous: HashMap<String, IndexedSession> = index
        .sessions
        .iter()
        .map(|session| (session.id.clone(), session.clone()))
        .collect();
    let plan = plan_sessions(store, &previous);
    if plan.writes.is_empty() && plan.remove.is_empty() {
        index.sessions = plan.sessions;
        return Ok(DeltaStats { reread: 0, docs: 0 });
    }
    let docs = plan.writes.iter().map(|write| write.docs.len() as u32).sum();
    let reread = plan.writes.len() as u32;
    {
        let mut writer = index
            .index
            .writer_with_num_threads(1, WRITER_HEAP)
            .map_err(|_| failed("could not build session index"))?;
        for id in plan.remove.iter().chain(plan.writes.iter().map(|write| &write.id)) {
            writer.delete_term(Term::from_field_text(index.fields.sid, id));
        }
        for write in &plan.writes {
            for chunk in &write.docs {
                writer
                    .add_document(doc!(
                        index.fields.title => write.title.as_str(),
                        index.fields.body => chunk.text.as_str(),
                        index.fields.sid => write.id.as_str(),
                        index.fields.role => chunk.role.as_str(),
                        index.fields.ts => chunk.ts,
                    ))
                    .map_err(|_| failed("could not index session"))?;
            }
        }
        writer.commit().map_err(|_| failed("could not commit session index"))?;
        // `IndexWriter`'s drop kills the merger. Wait, or deleted segments
        // stay in this process-lifetime index.
        writer
            .wait_merging_threads()
            .map_err(|_| failed("could not merge session index"))?;
    }
    if let Err(error) = index.reader.reload() {
        index.expected_docs = u64::MAX;
        let _ = error;
        return Err(failed("could not open session index"));
    }
    index.sessions = plan.sessions;
    index.expected_docs = index.reader.searcher().num_docs();
    Ok(DeltaStats { reread, docs })
}

fn plan_sessions(store: &OwnedSessionStore<'_>, previous: &HashMap<String, IndexedSession>) -> Plan {
    let mut bytes = 0usize;
    let mut sessions = Vec::new();
    let mut writes = Vec::new();
    let mut live = HashSet::new();
    for (id, mtime, len) in store.search_stamps().into_iter().take(MAX_SESSIONS) {
        if bytes > MAX_BYTES {
            break;
        }
        if let Some(old) = previous.get(&id) {
            if old.mtime == mtime && old.len == len {
                // An uncut session stays the same documents wherever it sits under the cap.
                let still_whole = !old.truncated && bytes.saturating_add(old.counted_bytes) <= MAX_BYTES;
                if still_whole || old.bytes_before == bytes {
                    let mut kept = old.clone();
                    kept.bytes_before = bytes;
                    bytes = bytes.saturating_add(old.counted_bytes);
                    live.insert(id);
                    sessions.push(kept);
                    if bytes > MAX_BYTES {
                        break;
                    }
                    continue;
                }
            }
        }
        let Ok(session) = store.get(&id) else {
            continue;
        };
        let bytes_before = bytes;
        let (counted, docs, truncated) = chunk_session(&session, bytes_before);
        bytes = bytes.saturating_add(counted);
        live.insert(session.session_id.clone());
        sessions.push(IndexedSession {
            id: session.session_id.clone(),
            mtime,
            len,
            counted_bytes: counted,
            bytes_before,
            truncated,
        });
        writes.push(WriteSession {
            id: session.session_id,
            title: session.title,
            docs,
        });
        if bytes > MAX_BYTES {
            break;
        }
    }
    let remove = previous.keys().filter(|id| !live.contains(*id)).cloned().collect();
    Plan {
        sessions,
        writes,
        remove,
    }
}

fn chunk_session(session: &Session, bytes_before: usize) -> (usize, Vec<StoredChunk>, bool) {
    let mut counted = 0usize;
    let mut bytes = bytes_before;
    let mut docs = Vec::new();
    let mut truncated = false;
    for message in &session.messages {
        bytes = bytes.saturating_add(message.text.len());
        counted = counted.saturating_add(message.text.len());
        if bytes > MAX_BYTES {
            truncated = true;
            break;
        }
        for text in chunks(&message.text, INDEX_OVERLAP) {
            docs.push(StoredChunk {
                text,
                role: message.role.clone(),
                ts: message.ts,
            });
        }
    }
    (counted, docs, truncated)
}

fn run_query(
    index: &OwnerIndex,
    expression: &str,
    query: &str,
    lower_query: &str,
    overlap: usize,
) -> Result<Vec<SessionHit>> {
    if index.expected_docs == 0 {
        return Ok(Vec::new());
    }
    let searcher = index.reader.searcher();
    let parser = QueryParser::for_index(&index.index, vec![index.fields.title, index.fields.body]);
    let parsed = parser
        .parse_query(expression)
        .map_err(|_| failed("query cannot be indexed"))?;
    // A short query can match only in the chunk tail. Those documents are
    // discarded below, and they outscore the chunk that owns the same text.
    // Cap hits after that discard, not before it.
    let limit = usize::try_from(searcher.num_docs()).unwrap_or(usize::MAX).max(1);
    let top = searcher
        .search(&parsed, &TopDocs::with_limit(limit).order_by_score())
        .map_err(|_| failed("session search failed"))?;

    let mut hits = Vec::new();
    let mut per_session = std::collections::HashMap::<String, usize>::new();
    for (score, addr) in top {
        let document: TantivyDocument = searcher.doc(addr).map_err(|_| failed("hit unreadable"))?;
        let Some(stored) = document.get_first(index.fields.body).and_then(|value| value.as_str()) else {
            continue;
        };
        let text = trim_to_query(stored, overlap);
        let Some(match_start) = text.to_lowercase().find(lower_query) else {
            continue;
        };
        let owned_bytes = text
            .chars()
            .take(CHUNK_CHARS)
            .flat_map(char::to_lowercase)
            .map(char::len_utf8)
            .sum::<usize>();
        if match_start >= owned_bytes {
            continue;
        }
        let Some(sid) = document.get_first(index.fields.sid).and_then(|value| value.as_str()) else {
            continue;
        };
        let n = per_session.entry(sid.to_string()).or_insert(0);
        if *n >= PER_SESSION {
            continue;
        }
        *n += 1;
        hits.push(SessionHit {
            session_id: sid.to_string(),
            title: document
                .get_first(index.fields.title)
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            snippet: snippet(text, query),
            role: document
                .get_first(index.fields.role)
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            ts: document
                .get_first(index.fields.ts)
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0),
            score,
        });
        if hits.len() >= MAX_HITS {
            break;
        }
    }
    Ok(hits)
}

/// The indexed chunk is longer than a per-query chunk. Keep the prefix a
/// query of this overlap would have indexed, so the literal check and the
/// snippet see the same characters as before.
fn trim_to_query(text: &str, overlap: usize) -> &str {
    match text.char_indices().nth(CHUNK_CHARS + overlap) {
        Some((byte, _)) => &text[..byte],
        None => text,
    }
}

fn chunks(text: &str, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    (0..chars.len())
        .step_by(CHUNK_CHARS)
        .map(|start| {
            chars[start..(start + CHUNK_CHARS + overlap).min(chars.len())]
                .iter()
                .collect()
        })
        .collect()
}

fn snippet(text: &str, query: &str) -> String {
    let lower = text.to_lowercase();
    let q = query.to_lowercase();
    let idx = lower.find(&q).unwrap_or(0);
    // Map lowercase byte offsets back to original characters, including expansions.
    let mut lowered_bytes = 0;
    let match_char = text
        .chars()
        .take_while(|c| {
            lowered_bytes += c.to_lowercase().map(char::len_utf8).sum::<usize>();
            lowered_bytes <= idx
        })
        .count();
    text.chars()
        .skip(match_char.saturating_sub(40))
        .take(SNIPPET_CHARS)
        .collect()
}

#[cfg(test)]
fn testing_forget_session_index(dir: &Path, owner: &str) {
    let mut map = cache().lock().unwrap_or_else(|poison| poison.into_inner());
    map.remove(&(dir.to_path_buf(), owner.to_string()));
}

#[cfg(test)]
fn testing_segment_count(dir: &Path, owner: &str) -> usize {
    let slot = slot_for(dir, owner);
    let guard = slot.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    guard
        .as_ref()
        .and_then(|index| index.index.searchable_segments().ok())
        .map(|segments| segments.len())
        .unwrap_or(0)
}

#[cfg(test)]
fn testing_corrupt_session_index(dir: &Path, owner: &str) {
    let slot = slot_for(dir, owner);
    let mut guard = slot.inner.lock().unwrap_or_else(|poison| poison.into_inner());
    let current = guard.as_mut().expect("cached index");
    let expected = current.expected_docs.max(1);
    let mut blank = OwnerIndex::empty().expect("replacement index");
    blank.expected_docs = expected;
    *current = blank;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::sessions::{SessionStore, TokenTally};
    use std::hint::black_box;

    /// The old per-search index kept this many BM25 candidates. The bench
    /// still does. Production walks every live document, then caps hits.
    const CANDIDATES: usize = 64;
    use std::time::Instant;

    fn say(owned: &OwnedSessionStore<'_>, session_id: &str, text: &str) {
        owned
            .record_exchange(session_id, text, "reply", "test", &TokenTally::default(), &[])
            .unwrap();
    }

    #[test]
    fn search_finds_literal_phrases_across_transcript_chunk_boundaries() {
        for query in [
            "needle across boundary".to_string(),
            format!("{}edge", "needle ".repeat(28)),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = SessionStore::new(dir.path()).unwrap();
            let owned = store.for_owner("user_search_boundary");
            let session = owned.create("test", "boundary fixture").unwrap();
            let text = format!("{} {query}", "界 ".repeat(599));
            say(&owned, &session.session_id, &text);

            let hits = search_sessions(&owned, &query).unwrap();
            assert_eq!(hits.len(), 1, "expected one hit for {query:?}, got {hits:?}");
            assert_eq!(hits[0].session_id, session.session_id);
            assert_eq!(hits[0].role, "user");
            assert!(hits[0].snippet.contains("needle"), "{hits:?}");
        }
    }

    #[test]
    fn one_index_answers_a_longer_boundary_query_after_a_shorter_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let owned = store.for_owner("user_search_boundary");
        let session = owned.create("test", "boundary fixture").unwrap();
        let long = format!("{}edge", "needle ".repeat(28));
        assert_eq!(long.chars().count(), MAX_QUERY_CHARS);
        say(&owned, &session.session_id, &format!("{} {long}", "界 ".repeat(599)));

        let short = search_sessions(&owned, "needle").unwrap();
        assert!(!short.is_empty() && short.len() <= PER_SESSION, "{short:?}");
        assert!(short.iter().all(|hit| hit.session_id == session.session_id));
        let again = search_sessions(&owned, &long).unwrap();
        assert_eq!(
            again.len(),
            1,
            "a longer boundary phrase must still hit the cached index, got {again:?}"
        );
        assert_eq!(again[0].session_id, session.session_id);
        assert!(
            !testing_trace().rebuild,
            "the longer query must reuse the index built for the shorter one"
        );
    }

    #[test]
    fn search_returns_one_hit_for_a_single_case_expanded_boundary_match() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let owned = store.for_owner("user_search_boundary");
        let session = owned.create("test", "boundary fixture").unwrap();
        let text = format!("{}İneedle", "界 ".repeat(600));
        say(&owned, &session.session_id, &text);

        let hits = search_sessions(&owned, "İneedle").unwrap();
        assert_eq!(hits.len(), 1, "a single occurrence must yield one hit, got {hits:?}");
        assert_eq!(hits[0].session_id, session.session_id);
        assert!(hits[0].snippet.contains("İneedle"), "{hits:?}");
    }

    #[test]
    fn indexed_overlap_covers_the_longest_lowercased_query() {
        let expanded = "İ".repeat(MAX_QUERY_CHARS);
        let overlap = expanded.to_lowercase().chars().count() - 1;
        assert!(overlap <= INDEX_OVERLAP, "overlap {overlap} exceeds {INDEX_OVERLAP}");
        assert!(char::MAX.to_lowercase().count() <= 3);
    }

    #[test]
    fn snippets_preserve_unicode_and_locate_case_expanded_matches() {
        for prefix in ["é".repeat(21), "界".repeat(21), "🙂".repeat(21)] {
            let text = format!("{prefix} needle");
            assert_eq!(snippet(&text, "NEEDLE"), text);
        }

        // İ expands when lowercased; ΟΣ needs contextual final-sigma casing.
        let text = format!("{} ΟΣ {}", "İ".repeat(200), "z".repeat(200));
        let expected = format!("{} ΟΣ {}", "İ".repeat(39), "z".repeat(117));
        assert_eq!(snippet(&text, "ος"), expected);
        assert_eq!(snippet(&text, "ΟΣ").chars().count(), SNIPPET_CHARS);
    }

    #[test]
    fn search_finds_a_session_added_after_the_index_exists() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let owned = store.for_owner("user_search_add");
        assert!(search_sessions(&owned, "firstneedle").unwrap().is_empty());

        let first = owned.create("test", "one").unwrap();
        say(&owned, &first.session_id, "alpha firstneedle beta");
        let found = search_sessions(&owned, "firstneedle").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].session_id, first.session_id);

        let second = owned.create("test", "two").unwrap();
        say(&owned, &second.session_id, "gamma secondneedle delta");
        let both_old = search_sessions(&owned, "firstneedle").unwrap();
        assert_eq!(
            testing_trace().reread,
            1,
            "adding one session must not reread the other"
        );
        let added = search_sessions(&owned, "secondneedle").unwrap();
        assert_eq!(both_old.len(), 1);
        assert_eq!(both_old[0].session_id, first.session_id);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].session_id, second.session_id);
        assert_eq!(testing_trace().reread, 0);
    }

    #[test]
    fn search_drops_a_deleted_session_and_keeps_the_other() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let owned = store.for_owner("user_search_delete");
        let kept = owned.create("test", "kept").unwrap();
        let gone = owned.create("test", "gone").unwrap();
        say(&owned, &kept.session_id, "keepneedle stays");
        say(&owned, &gone.session_id, "goneneedle leaves");
        assert_eq!(search_sessions(&owned, "goneneedle").unwrap().len(), 1);

        std::fs::remove_file(dir.path().join(format!("{}.json", gone.session_id))).unwrap();
        assert!(search_sessions(&owned, "goneneedle").unwrap().is_empty());
        assert_eq!(
            search_sessions(&owned, "keepneedle").unwrap()[0].session_id,
            kept.session_id
        );

        owned.clear().unwrap();
        assert!(search_sessions(&owned, "keepneedle").unwrap().is_empty());
    }

    #[test]
    fn search_reflects_an_edited_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let owned = store.for_owner("user_search_edit");
        let session = owned.create("test", "edit").unwrap();
        say(&owned, &session.session_id, "beforeeditneedle original");
        assert_eq!(search_sessions(&owned, "beforeeditneedle").unwrap().len(), 1);

        let mut edited = owned.get(&session.session_id).unwrap();
        edited.messages[0].text = "aftereditneedle replacement".to_string();
        let path = dir.path().join(format!("{}.json", session.session_id));
        std::fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();

        assert!(
            search_sessions(&owned, "beforeeditneedle").unwrap().is_empty(),
            "the old phrase must leave the index"
        );
        let hits = search_sessions(&owned, "aftereditneedle").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, session.session_id);
        assert!(hits[0].snippet.contains("aftereditneedle"), "{hits:?}");
    }

    #[test]
    fn search_sees_a_same_length_edit() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let owned = store.for_owner("user_search_edit");
        let session = owned.create("test", "edit").unwrap();
        say(&owned, &session.session_id, "sameeditneedleAAAA");
        assert_eq!(search_sessions(&owned, "sameeditneedleAAAA").unwrap().len(), 1);

        let mut edited = owned.get(&session.session_id).unwrap();
        edited.messages[0].text = "sameeditneedleBBBB".to_string();
        assert_eq!(edited.messages[0].text.len(), "sameeditneedleAAAA".len());
        std::fs::write(
            dir.path().join(format!("{}.json", session.session_id)),
            serde_json::to_vec(&edited).unwrap(),
        )
        .unwrap();

        assert!(search_sessions(&owned, "sameeditneedleAAAA").unwrap().is_empty());
        assert_eq!(search_sessions(&owned, "sameeditneedleBBBB").unwrap().len(), 1);
    }

    #[test]
    fn concurrent_searches_return_the_owners_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let owned = store.for_owner("user_search_race");
        let session = owned.create("test", "race").unwrap();
        say(&owned, &session.session_id, "raceneedle");
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..20 {
                        let hits = search_sessions(&owned, "raceneedle").unwrap();
                        assert_eq!(hits.len(), 1);
                        assert_eq!(hits[0].session_id, session.session_id);
                    }
                });
            }
        });
    }

    #[test]
    fn search_rebuilds_a_missing_or_corrupt_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let owner = "user_search_recover";
        let owned = store.for_owner(owner);
        let session = owned.create("test", "recover").unwrap();
        say(&owned, &session.session_id, "recoverneedle stays findable");
        assert_eq!(search_sessions(&owned, "recoverneedle").unwrap().len(), 1);

        testing_corrupt_session_index(dir.path(), owner);
        let after_corrupt = search_sessions(&owned, "recoverneedle").unwrap();
        assert_eq!(
            after_corrupt.len(),
            1,
            "a corrupt index must be rebuilt, got {after_corrupt:?}"
        );
        assert!(testing_trace().rebuild);
        assert_eq!(after_corrupt[0].session_id, session.session_id);

        testing_forget_session_index(dir.path(), owner);
        let after_missing = search_sessions(&owned, "recoverneedle").unwrap();
        assert_eq!(
            after_missing.len(),
            1,
            "a missing index must be rebuilt, got {after_missing:?}"
        );
        assert!(testing_trace().rebuild);
        assert_eq!(after_missing[0].session_id, session.session_id);
    }

    #[test]
    fn overlap_tails_do_not_hide_a_real_needle() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let owned = store.for_owner("user_search_overlap_crowd");
        let heavy = format!(
            "{}{}{}",
            "x".repeat(CHUNK_CHARS),
            " needle ".repeat(40),
            "z ".repeat(1_000)
        );
        for n in 0..CANDIDATES {
            let session = owned.create("test", &format!("heavy {n}")).unwrap();
            say(&owned, &session.session_id, &heavy);
        }
        let hits = search_sessions(&owned, "needle").unwrap();
        assert!(
            !hits.is_empty(),
            "a needle past the first chunk must survive the candidate walk"
        );
        assert!(hits.iter().all(|hit| hit.snippet.to_lowercase().contains("needle")));
    }

    #[test]
    fn repeated_edits_merge_segments() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let owner = "user_search_merge";
        let owned = store.for_owner(owner);
        let session = owned.create("test", "merge").unwrap();
        say(&owned, &session.session_id, "mergeneedle seed");
        let updates = 12;
        for n in 0..updates {
            let mut edited = owned.get(&session.session_id).unwrap();
            edited.messages[0].text = format!("mergeneedle {n} {}", "m".repeat(8_000));
            std::fs::write(
                dir.path().join(format!("{}.json", session.session_id)),
                serde_json::to_vec(&edited).unwrap(),
            )
            .unwrap();
            assert_eq!(search_sessions(&owned, &format!("mergeneedle {n}")).unwrap().len(), 1);
        }
        let segments = testing_segment_count(dir.path(), owner);
        assert!(
            segments < updates,
            "merges should collapse commits, found {segments} segments after {updates} edits"
        );
    }

    #[test]
    fn search_does_not_return_another_owners_sessions_or_write_an_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let alice = store.for_owner("user_search_alice");
        let bob = store.for_owner("user_search_bob");
        let alice_session = alice.create("test", "alice").unwrap();
        let bob_session = bob.create("test", "bob").unwrap();
        say(&alice, &alice_session.session_id, "aliceprivate needle");
        say(&bob, &bob_session.session_id, "bobprivate needle");

        assert!(search_sessions(&alice, "bobprivate").unwrap().is_empty());
        assert_eq!(
            search_sessions(&bob, "bobprivate").unwrap()[0].session_id,
            bob_session.session_id
        );
        assert_eq!(
            search_sessions(&alice, "aliceprivate").unwrap()[0].session_id,
            alice_session.session_id
        );

        std::fs::remove_file(dir.path().join(format!("{}.json", bob_session.session_id))).unwrap();
        assert!(search_sessions(&bob, "bobprivate").unwrap().is_empty());
        assert_eq!(search_sessions(&alice, "aliceprivate").unwrap().len(), 1);

        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|name| name.ends_with(".json")),
            "session search must not leave an index on disk, found {names:?}"
        );
    }

    #[test]
    fn byte_budget_follows_the_newest_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let owned = store.for_owner("user_search_budget");
        let older = owned.create("test", "older").unwrap();
        say(
            &owned,
            &older.session_id,
            &format!("olderbudgetneedle {}", "a".repeat(1_200_000)),
        );
        let newer = owned.create("test", "newer").unwrap();
        say(
            &owned,
            &newer.session_id,
            &format!("newerbudgetneedle {}", "b".repeat(1_200_000)),
        );

        assert_eq!(search_sessions(&owned, "newerbudgetneedle").unwrap().len(), 1);
        assert!(
            search_sessions(&owned, "olderbudgetneedle").unwrap().is_empty(),
            "the older session sits past the byte cap"
        );

        let mut shrunk = owned.get(&newer.session_id).unwrap();
        shrunk.messages[0].text = "newerbudgetneedle short".to_string();
        std::fs::write(
            dir.path().join(format!("{}.json", newer.session_id)),
            serde_json::to_vec(&shrunk).unwrap(),
        )
        .unwrap();
        assert_eq!(search_sessions(&owned, "olderbudgetneedle").unwrap().len(), 1);
        assert_eq!(search_sessions(&owned, "newerbudgetneedle").unwrap().len(), 1);
    }

    #[test]
    fn unchanged_corpus_is_not_indexed_again() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let owned = store.for_owner("user_search_warm");
        let session = owned.create("test", "warm").unwrap();
        say(&owned, &session.session_id, "warmneedle once");
        assert_eq!(search_sessions(&owned, "warmneedle").unwrap().len(), 1);
        assert!(testing_trace().rebuild);
        assert!(testing_trace().reread >= 1);

        assert_eq!(search_sessions(&owned, "warmneedle").unwrap().len(), 1);
        let trace = testing_trace();
        assert!(!trace.rebuild, "the second search must keep the index");
        assert_eq!(trace.reread, 0);
        assert_eq!(trace.docs, 0);
    }

    /// The pre-change search: a new RAM index on every call, chunked for that query.
    fn search_rebuilding(store: &OwnedSessionStore<'_>, query: &str) -> Result<Vec<SessionHit>> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > MAX_QUERY_CHARS {
            return Ok(Vec::new());
        }
        let Some(expression) = crate::server::retrieval_ranking::tantivy_expression(query) else {
            return Ok(Vec::new());
        };
        let lower_query = query.to_lowercase();
        let overlap = lower_query.chars().count() - 1;
        let mut schema_builder = Schema::builder();
        let text_options = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("en_stem")
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );
        let stored = TextOptions::default().set_stored();
        let f_title = schema_builder.add_text_field("title", text_options.clone());
        let f_body = schema_builder.add_text_field("text", text_options);
        let f_sid = schema_builder.add_text_field("session_id", stored.clone());
        let f_role = schema_builder.add_text_field("role", stored);
        let f_ts = schema_builder.add_f64_field("ts", STORED);
        let f_row = schema_builder.add_u64_field("row", STORED);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        let mut writer = index
            .writer_with_num_threads(1, WRITER_HEAP)
            .map_err(|_| failed("could not build session index"))?;
        let summaries = store.list();
        let mut evidence: Vec<(String, String, String, f64, String)> = Vec::new();
        let mut bytes = 0usize;
        for summary in summaries.iter().take(MAX_SESSIONS) {
            let Some(id) = summary["session_id"].as_str() else {
                continue;
            };
            let Ok(session) = store.get(id) else { continue };
            let title = session.title.clone();
            for msg in &session.messages {
                bytes = bytes.saturating_add(msg.text.len());
                if bytes > MAX_BYTES {
                    break;
                }
                for chunk in chunks(&msg.text, overlap) {
                    let row = evidence.len() as u64;
                    writer
                        .add_document(doc!(
                            f_title => title.as_str(),
                            f_body => chunk.as_str(),
                            f_sid => session.session_id.as_str(),
                            f_role => msg.role.as_str(),
                            f_ts => msg.ts,
                            f_row => row,
                        ))
                        .map_err(|_| failed("could not index session"))?;
                    evidence.push((
                        session.session_id.clone(),
                        title.clone(),
                        msg.role.clone(),
                        msg.ts,
                        chunk,
                    ));
                }
            }
            if bytes > MAX_BYTES {
                break;
            }
        }
        writer.commit().map_err(|_| failed("could not commit session index"))?;
        if evidence.is_empty() {
            return Ok(Vec::new());
        }
        let reader = index.reader().map_err(|_| failed("could not open session index"))?;
        let searcher = reader.searcher();
        let parser = QueryParser::for_index(&index, vec![f_title, f_body]);
        let parsed = parser
            .parse_query(&expression)
            .map_err(|_| failed("query cannot be indexed"))?;
        let top = searcher
            .search(&parsed, &TopDocs::with_limit(CANDIDATES).order_by_score())
            .map_err(|_| failed("session search failed"))?;
        let mut hits = Vec::new();
        let mut per_session = HashMap::<String, usize>::new();
        for (score, addr) in top {
            let document: TantivyDocument = searcher.doc(addr).map_err(|_| failed("hit unreadable"))?;
            let row = document.get_first(f_row).and_then(|value| value.as_u64()).unwrap_or(0) as usize;
            let Some((sid, title, role, ts, text)) = evidence.get(row) else {
                continue;
            };
            let Some(match_start) = text.to_lowercase().find(&lower_query) else {
                continue;
            };
            let owned_bytes = text
                .chars()
                .take(CHUNK_CHARS)
                .flat_map(char::to_lowercase)
                .map(char::len_utf8)
                .sum::<usize>();
            if match_start >= owned_bytes {
                continue;
            }
            let n = per_session.entry(sid.clone()).or_insert(0);
            if *n >= PER_SESSION {
                continue;
            }
            *n += 1;
            hits.push(SessionHit {
                session_id: sid.clone(),
                title: title.clone(),
                snippet: snippet(text, query),
                role: role.clone(),
                ts: *ts,
                score,
            });
            if hits.len() >= MAX_HITS {
                break;
            }
        }
        Ok(hits)
    }

    fn percentile(samples: &[u128], p: f64) -> u128 {
        let mut ordered = samples.to_vec();
        ordered.sort_unstable();
        let rank = ((ordered.len() as f64) * p).ceil() as usize;
        ordered[rank.saturating_sub(1).min(ordered.len() - 1)]
    }

    fn summarize(label: &str, samples: &[u128], hits: usize) {
        let mut ordered = samples.to_vec();
        ordered.sort_unstable();
        let median = percentile(samples, 0.50);
        let p95 = percentile(samples, 0.95);
        println!(
            "{label} n={} median_us={} p95_us={} min_us={} max_us={} hits={}",
            samples.len(),
            median / 1000,
            p95 / 1000,
            ordered[0] / 1000,
            ordered[ordered.len() - 1] / 1000,
            hits
        );
    }

    fn fill_corpus(owned: &OwnedSessionStore<'_>, sessions: usize, chars: usize, token: &str) {
        for index in 0..sessions {
            let session = owned.create("bench", &format!("session {index}")).unwrap();
            let mut text = "commonword ".repeat(chars / "commonword ".len());
            if index == 0 {
                text.push_str(token);
            }
            say(owned, &session.session_id, &text);
        }
    }

    #[test]
    #[ignore = "manual before/after latency comparison; not part of CI"]
    fn session_search_latency_bench() {
        let model = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("model name"))
                    .and_then(|line| line.split(':').nth(1))
                    .map(|value| value.trim().to_string())
            })
            .unwrap_or_else(|| "unknown".to_string());
        let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
        println!(
            "cpu={model} cores={} load={}",
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
            load.trim()
        );
        for (name, sessions, chars, token) in [
            ("small", 20usize, 2_000usize, "smalltoken"),
            ("large", 200, 8_000, "largetoken"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = SessionStore::new(dir.path()).unwrap();
            let owner = "user_search_bench";
            let owned = store.for_owner(owner);
            fill_corpus(&owned, sessions, chars, token);
            let selective = search_sessions(&owned, token).unwrap();
            let selective_old = search_rebuilding(&owned, token).unwrap();
            assert_eq!(selective.len(), 1, "{name} cached selective");
            assert_eq!(selective_old.len(), 1, "{name} rebuild selective");
            assert_eq!(selective[0].session_id, selective_old[0].session_id);
            assert_eq!(selective[0].snippet, selective_old[0].snippet, "{name} snippet");
            let old_common = search_rebuilding(&owned, "commonword").unwrap();
            let new_common = search_sessions(&owned, "commonword").unwrap();
            let old_ids: std::collections::BTreeSet<_> = old_common.iter().map(|hit| hit.session_id.as_str()).collect();
            let new_ids: std::collections::BTreeSet<_> = new_common.iter().map(|hit| hit.session_id.as_str()).collect();
            println!(
                "corpus={name} commonword_same_ids={} rebuild_ids={} cached_ids={}",
                old_ids == new_ids,
                old_ids.len(),
                new_ids.len()
            );

            let mut rebuild_us = Vec::new();
            let mut cached_us = Vec::new();
            let mut prepare_us = Vec::new();
            let mut query_us = Vec::new();
            let cached_hits = new_common.len();
            for _ in 0..20 {
                let started = Instant::now();
                let hits = search_rebuilding(&owned, "commonword").unwrap();
                rebuild_us.push(started.elapsed().as_nanos());
                assert!(!hits.is_empty());
                black_box(&hits);

                let started = Instant::now();
                let hits = search_sessions(&owned, "commonword").unwrap();
                cached_us.push(started.elapsed().as_nanos());
                let warm = testing_trace();
                prepare_us.push(warm.prepare_ns);
                query_us.push(warm.query_ns);
                assert!(
                    !warm.rebuild && warm.reread == 0 && warm.docs == 0,
                    "{name} warm indexed again: {warm:?}"
                );
                assert!(!hits.is_empty());
                black_box(&hits);
            }
            println!("corpus={name} sessions={sessions} chars_per_session={chars}");
            summarize("rebuild_every_search", &rebuild_us, cached_hits);
            summarize("cached_incremental", &cached_us, cached_hits);
            summarize("cached_prepare", &prepare_us, cached_hits);
            summarize("cached_query", &query_us, cached_hits);

            let mut cold_us = Vec::new();
            let mut cold_prepare = Vec::new();
            let mut cold_query = Vec::new();
            for _ in 0..5 {
                testing_forget_session_index(dir.path(), owner);
                let started = Instant::now();
                let hits = search_sessions(&owned, "commonword").unwrap();
                cold_us.push(started.elapsed().as_nanos());
                let cold = testing_trace();
                cold_prepare.push(cold.prepare_ns);
                cold_query.push(cold.query_ns);
                assert!(!hits.is_empty());
                assert!(cold.rebuild && cold.reread > 0, "{name} cold trace {cold:?}");
            }
            summarize("cached_cold_build", &cold_us, cached_hits);
            summarize("cold_prepare", &cold_prepare, cached_hits);
            summarize("cold_query", &cold_query, cached_hits);
        }
    }
}
