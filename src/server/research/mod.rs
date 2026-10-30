//! Operator research corpus: hybrid retrieval in one owner-only SQLite file,
//! ported from cg-researcher's `rag.rs` and adapted to harness error types and
//! the `research.*` config section.
//!
//! * keyword: SQLite FTS5 BM25 (part of the bundled SQLite core),
//! * semantic: exact cosine over unit-normalized f32 vectors, scanned in RAM
//!   by a cache-friendly Rust loop (LLVM vectorizes it to NEON / AVX),
//! * fused with RRF (k = 60), the same shape as CyClaw's Chroma + BM25 + RRF.
//!
//! No vector server, no extension, no C beyond SQLite itself, no `unsafe`.
//! Exact search is the right call at personal-corpus scale: 100k chunks of
//! 384-d vectors is ~150 MB and a full scan takes tens of milliseconds.
//!
//! Keyword queries hit the live database. The vector matrix is reloaded when
//! the index `generation` changes, so a confirm-gated `/api/research/reindex`
//! is picked up by a running server without a restart. The corpus directory is
//! read-only: the walk never follows symlinks and reads only `.md`,
//! `.markdown`, `.txt` and `.docx` files within the configured byte caps.
//! Embeddings are optional and loopback-only (see [`ResearchConfig`]); an
//! embedder failure degrades a reindex to keyword-only, never to a write or a
//! network retry outside the configured endpoint.

mod config;
mod embed;

pub use config::ResearchConfig;
pub use embed::{EmbedClient, EMBED_FAILED};

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use rusqlite::{params, Connection, OptionalExtension};

use crate::common::errors::{HarnessError, Result};
use crate::server::routes::structured_memory::off_worker;

pub const RRF_K: f64 = 60.0;
/// Bump when the on-disk layout changes; older indexes are dropped and must
/// be rebuilt with `/api/research/reindex`.
pub const SCHEMA_VERSION: &str = "2";
/// Largest embedding we accept (matches common model ceilings, bounds RAM).
pub const MAX_DIMENSIONS: usize = 8192;
const CANDIDATES: usize = 50;
/// SQLite/IO failures inside the store surface under this code.
pub const RESEARCH_IO: &str = "RESEARCH_IO";
/// The corpus directory is unset, missing, or unreadable as a directory.
pub const RESEARCH_NO_CORPUS: &str = "RESEARCH_NO_CORPUS";
/// The FTS5 table, shared by `prepare` and the drop-and-recreate in `write_all`.
const CREATE_FTS: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts
           USING fts5(text, tokenize = 'unicode61 remove_diacritics 2');";

#[derive(Debug, Clone)]
pub struct Chunk {
    pub id: i64,
    pub source: String,
    pub text: String,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub source: String,
    pub text: String,
    pub rrf: f64,
    pub cosine: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct Retrieval {
    pub hits: Vec<Hit>,
    pub top_score: f64,
    pub top_cosine: Option<f64>,
    /// Number of ranked lists that contributed (1 = BM25 only).
    pub lists: usize,
}

fn research_io(context: &str, err: impl std::fmt::Display) -> HarnessError {
    HarnessError::new(RESEARCH_IO, format!("{context}: {err}"))
}

pub fn tokenize(text: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "a", "an", "and", "or", "of", "to", "in", "is", "it", "for", "on", "with", "as", "at", "by", "be",
        "this", "that", "what", "how", "my", "me", "i", "you", "are", "was", "do", "does",
    ];
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1)
        .map(|t| t.to_lowercase())
        .filter(|t| !STOP.contains(&t.as_str()))
        .collect()
}

pub fn chunk_text(text: &str, size: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + size).min(chars.len());
        // Prefer to break on whitespace.
        let break_at = if end < chars.len() {
            chars[start..end]
                .iter()
                .rposition(|c| c.is_whitespace())
                .filter(|ws| *ws > size / 2)
        } else {
            None
        };
        if let Some(ws) = break_at {
            end = start + ws;
        }
        let piece: String = chars[start..end].iter().collect();
        if !piece.trim().is_empty() {
            out.push(piece.trim().to_string());
        }
        if end == chars.len() {
            break;
        }
        start = end.saturating_sub(overlap).max(start + 1);
    }
    out
}

/// Unit-normalize `v` for storage/query, or `None` if it is unusable
/// (wrong size, non-finite, or zero). Non-finite values would turn every
/// score into NaN and poison ranking.
pub fn normalize(v: &[f32]) -> Option<Vec<f32>> {
    if !(1..=MAX_DIMENSIONS).contains(&v.len()) || !v.iter().all(|x| x.is_finite()) {
        return None;
    }
    let n = dot(v, v).sqrt();
    (n.is_finite() && n > 0.0).then(|| v.iter().map(|x| x / n).collect())
}

/// Dot product with 8 independent accumulators so LLVM can vectorize it.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let (ca, ra) = a.as_chunks::<8>();
    let (cb, rb) = b.as_chunks::<8>();
    let mut acc = [0f32; 8];
    for (x, y) in ca.iter().zip(cb) {
        for i in 0..8 {
            acc[i] += x[i] * y[i];
        }
    }
    let mut s: f32 = acc.iter().sum();
    for (x, y) in ra.iter().zip(rb) {
        s += x * y;
    }
    s
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>().0.iter().map(|x| f32::from_le_bytes(*x)).collect()
}

/// Fuse ranked lists of ids with reciprocal-rank fusion. Ties break on the
/// smaller id so results are deterministic.
pub fn rrf<T: Copy + Eq + std::hash::Hash + Ord>(lists: &[Vec<T>]) -> Vec<(T, f64)> {
    let mut acc: std::collections::HashMap<T, f64> = std::collections::HashMap::new();
    for list in lists {
        for (rank, id) in list.iter().enumerate() {
            *acc.entry(*id).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
    }
    let mut v: Vec<(T, f64)> = acc.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// FTS5 MATCH expression from our tokenizer: every term double-quoted (so
/// FTS5 syntax in user text such as `NEAR`, `*`, `:` or `^` is inert),
/// OR-joined. `None` if nothing is left after stop-words.
pub fn fts_query(query: &str) -> Option<String> {
    let mut terms = tokenize(query);
    terms.sort();
    terms.dedup();
    if terms.is_empty() {
        return None;
    }
    Some(
        terms
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

/// Ensure the schema on `c`, dropping a pre-v2 layout.
fn prepare(c: &Connection) -> rusqlite::Result<()> {
    let has_meta: bool = c.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |r| r.get::<_, i64>(0).map(|n| n > 0),
    )?;
    let current: Option<String> = if has_meta {
        c.query_row("SELECT v FROM meta WHERE k = 'schema'", [], |r| r.get(0))
            .optional()?
    } else {
        None
    };
    if current.as_deref() != Some(SCHEMA_VERSION) {
        c.execute_batch(
            "DROP TABLE IF EXISTS chunks;
             DROP TABLE IF EXISTS chunks_fts;
             DROP TABLE IF EXISTS meta;",
        )?;
    }
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS chunks(
           id INTEGER PRIMARY KEY, source TEXT NOT NULL, ord INTEGER NOT NULL,
           text TEXT NOT NULL, embedding BLOB);
         CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT NOT NULL);",
    )?;
    c.execute_batch(CREATE_FTS)?;
    c.execute(
        "INSERT OR IGNORE INTO meta(k, v) VALUES ('schema', ?1)",
        [SCHEMA_VERSION],
    )?;
    Ok(())
}

/// Replace all content in one transaction and bump `generation`. Vectors are
/// normalized; any whose dimension differs from the first usable one, or
/// that are non-finite/zero, are dropped (the chunk stays keyword-searchable).
/// Returns the number of stored vectors.
fn write_all(c: &mut Connection, rows: &[(String, i64, String, Option<Vec<f32>>)]) -> rusqlite::Result<usize> {
    let dim = rows
        .iter()
        .filter_map(|r| r.3.as_deref().and_then(normalize))
        .map(|v| v.len())
        .next();
    let tx = c.transaction()?;
    // Drop and recreate the FTS table: `DELETE FROM` on FTS5 re-tokenizes every
    // old row to write delete markers, then merges them into the new segments.
    tx.execute_batch("DELETE FROM chunks; DROP TABLE IF EXISTS chunks_fts;")?;
    tx.execute_batch(CREATE_FTS)?;
    let mut stored = 0;
    {
        let mut ins = tx.prepare("INSERT INTO chunks(id, source, ord, text, embedding) VALUES (?1, ?2, ?3, ?4, ?5)")?;
        let mut fts = tx.prepare("INSERT INTO chunks_fts(rowid, text) VALUES (?1, ?2)")?;
        for (i, (src, ord, text, emb)) in rows.iter().enumerate() {
            let id = i as i64 + 1;
            let blob = emb
                .as_deref()
                .and_then(normalize)
                .filter(|v| Some(v.len()) == dim)
                .map(|v| to_blob(&v));
            stored += usize::from(blob.is_some());
            ins.execute(params![id, src, ord, text, blob])?;
            fts.execute(params![id, text])?;
        }
    }
    let generation: i64 = tx
        .query_row("SELECT v FROM meta WHERE k = 'generation'", [], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    tx.execute(
        "INSERT OR REPLACE INTO meta(k, v) VALUES ('generation', ?1)",
        [(generation + 1).to_string()],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO meta(k, v) VALUES ('embed_dim', ?1)",
        [dim.unwrap_or(0).to_string()],
    )?;
    tx.commit()?;
    Ok(stored)
}

/// Create the index directory with operator-only permissions (0700 on unix).
fn private_parent(db: &Path) -> Result<()> {
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent).map_err(|e| research_io("cannot create the research index directory", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| research_io("cannot restrict the research index directory", e))?;
        }
    }
    Ok(())
}

pub struct Index {
    db: PathBuf,
}

impl Index {
    pub fn open(db: impl Into<PathBuf>) -> Result<Self> {
        let db = db.into();
        private_parent(&db)?;
        let c = Connection::open(&db).map_err(|e| research_io("cannot open the research index", e))?;
        c.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| research_io("cannot set the research index journal mode", e))?;
        prepare(&c).map_err(|e| research_io("cannot prepare the research index schema", e))?;
        drop(c);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| research_io("cannot restrict the research index file", e))?;
        }
        Ok(Self { db })
    }

    pub fn path(&self) -> &Path {
        &self.db
    }

    /// A retriever over this index file.
    pub fn retriever(&self) -> Result<Retriever> {
        Retriever::open(&self.db)
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) -> std::io::Result<()> {
    if depth > 8 || !dir.is_dir() {
        return Ok(());
    }
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let ft = e.file_type()?;
        if ft.is_symlink() {
            continue; // never follow links out of the corpus
        }
        let p = e.path();
        if ft.is_dir() {
            collect_files(&p, out, depth + 1)?;
        } else if matches!(
            p.extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("md" | "markdown" | "txt" | "docx")
        ) {
            out.push(p);
        }
    }
    out.sort();
    Ok(())
}

/// Read at most `max` bytes of one corpus file.
fn read_capped(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)?.take(max + 1).read_to_end(&mut buf)?;
    Ok(buf)
}

/// One chunk of a corpus file: (relative source path, ordinal, text).
type Piece = (String, usize, String);

/// Walk the corpus read-only and chunk every eligible file. Returns
/// (files read, pieces). Files over `max_file_bytes`,
/// unreadable files, and unparseable `.docx` files are skipped; collection
/// stops once `max_total_bytes` of file content has been accepted.
fn collect_pieces(
    corpus: &Path,
    size: usize,
    overlap: usize,
    max_file_bytes: u64,
    max_total_bytes: u64,
) -> Result<(usize, Vec<Piece>)> {
    if !corpus.is_dir() {
        return Err(HarnessError::new(
            RESEARCH_NO_CORPUS,
            "research.corpus_dir is not a readable directory",
        ));
    }
    let mut files = Vec::new();
    collect_files(corpus, &mut files, 0).map_err(|e| research_io("cannot walk the research corpus", e))?;
    let mut pieces: Vec<(String, usize, String)> = Vec::new();
    let mut total: u64 = 0;
    let mut read_files = 0;
    for f in &files {
        if total >= max_total_bytes {
            break;
        }
        // symlink_metadata never follows links: a link planted between the walk
        // and this read is skipped instead of escaped through.
        let Ok(meta) = std::fs::symlink_metadata(f) else {
            continue;
        };
        if !meta.is_file() || meta.len() > max_file_bytes {
            continue;
        }
        total = total.saturating_add(meta.len());
        let Ok(bytes) = read_capped(f, max_file_bytes) else {
            continue;
        };
        let text = match f
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("md" | "markdown" | "txt") => String::from_utf8_lossy(&bytes).into_owned(),
            Some("docx") => match crate::server::docx::extract(&bytes) {
                Ok(text) => text,
                Err(_) => continue,
            },
            _ => continue,
        };
        if text.trim().is_empty() {
            continue;
        }
        read_files += 1;
        let rel = f.strip_prefix(corpus).unwrap_or(f).to_string_lossy().into_owned();
        for (i, ch) in chunk_text(&text, size, overlap).into_iter().enumerate() {
            pieces.push((rel.clone(), i, ch));
        }
    }
    Ok((read_files, pieces))
}

/// Rewrite the index file in one transaction on a fresh write connection.
fn write_index(db: &Path, rows: &[(String, i64, String, Option<Vec<f32>>)], embed_model: &str) -> Result<usize> {
    let mut c = Connection::open(db).map_err(|e| research_io("cannot open the research index for writing", e))?;
    prepare(&c).map_err(|e| research_io("cannot prepare the research index schema", e))?;
    let stored = write_all(&mut c, rows).map_err(|e| research_io("cannot write the research index", e))?;
    c.execute(
        "INSERT OR REPLACE INTO meta(k, v) VALUES ('embed_model', ?1)",
        params![embed_model],
    )
    .map_err(|e| research_io("cannot record the research embedding model", e))?;
    Ok(stored)
}

/// Unit vectors packed row-major, plus their chunk ids.
#[derive(Default)]
struct Dense {
    generation: i64,
    dim: usize,
    ids: Vec<i64>,
    data: Vec<f32>,
}

impl Dense {
    fn load(c: &Connection, generation: i64) -> rusqlite::Result<Self> {
        let mut d = Dense {
            generation,
            ..Default::default()
        };
        let mut st = c.prepare("SELECT id, embedding FROM chunks WHERE embedding IS NOT NULL ORDER BY id")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
        for row in rows {
            let (id, blob) = row?;
            let v = from_blob(&blob);
            if d.dim == 0 {
                d.dim = v.len();
            }
            if v.len() == d.dim && d.dim > 0 {
                d.ids.push(id);
                d.data.extend_from_slice(&v);
            }
        }
        Ok(d)
    }

    /// Top `k` (id, cosine) by exact scan; `q` must be unit length.
    fn top(&self, q: &[f32], k: usize) -> Vec<(i64, f64)> {
        if self.dim == 0 || q.len() != self.dim {
            return Vec::new();
        }
        let mut scored: Vec<(i64, f32)> = self
            .data
            .chunks_exact(self.dim)
            .zip(&self.ids)
            .map(|(row, id)| (*id, dot(row, q)))
            .collect();
        let k = k.min(scored.len());
        if k == 0 {
            return Vec::new();
        }
        let cmp = |a: &(i64, f32), b: &(i64, f32)| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0));
        scored.select_nth_unstable_by(k - 1, cmp);
        scored.truncate(k);
        scored.sort_by(cmp);
        scored.into_iter().map(|(i, s)| (i, f64::from(s))).collect()
    }
}

/// Hybrid retriever over a SQLite connection (the index file, or an
/// in-memory database for tests and empty installs).
pub struct Retriever {
    conn: Mutex<Connection>,
    dense: RwLock<Arc<Dense>>,
}

impl Retriever {
    /// Read-only view of an index file.
    pub fn open(db: &Path) -> Result<Self> {
        let c = Connection::open(db).map_err(|e| research_io("cannot open the research index", e))?;
        prepare(&c).map_err(|e| research_io("cannot prepare the research index schema", e))?;
        c.execute_batch("PRAGMA query_only = 1;")
            .map_err(|e| research_io("cannot pin the research index read-only", e))?;
        Ok(Self::from_conn(c))
    }

    /// Empty in-memory retriever (no index yet).
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// In-memory retriever seeded with `chunks` (tests, fixtures).
    pub fn new(chunks: Vec<Chunk>) -> Self {
        let build = || -> rusqlite::Result<Connection> {
            let mut c = Connection::open_in_memory()?;
            prepare(&c)?;
            let rows: Vec<_> = chunks
                .into_iter()
                .map(|c| (c.source, 0i64, c.text, c.embedding))
                .collect();
            write_all(&mut c, &rows)?;
            Ok(c)
        };
        match build() {
            Ok(c) => Self::from_conn(c),
            Err(e) => {
                tracing::warn!("in-memory index failed: {e}");
                Self::from_conn(Connection::open_in_memory().expect("in-memory SQLite"))
            }
        }
    }

    fn from_conn(c: Connection) -> Self {
        Self {
            conn: Mutex::new(c),
            dense: RwLock::new(Arc::new(Dense {
                generation: -1,
                ..Default::default()
            })),
        }
    }

    pub fn len(&self) -> usize {
        let c = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        c.query_row("SELECT count(*) FROM chunks", [], |r| r.get::<_, i64>(0))
            .map_or(0, |n| usize::try_from(n).unwrap_or(0))
    }
    /// `EXISTS` stops at the first row; `count(*)` scans the whole table.
    pub fn is_empty(&self) -> bool {
        let c = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        !c.query_row("SELECT EXISTS(SELECT 1 FROM chunks)", [], |r| r.get::<_, bool>(0))
            .unwrap_or(false)
    }

    /// `(embed_model, embed_dim)` recorded by the last reindex, if any.
    pub fn embed_meta(&self) -> (Option<String>, usize) {
        let c = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let get = |k: &str| {
            c.query_row("SELECT v FROM meta WHERE k = ?1", [k], |r| r.get::<_, String>(0))
                .optional()
                .ok()
                .flatten()
        };
        let dim = get("embed_dim").and_then(|v| v.parse().ok()).unwrap_or(0);
        (get("embed_model"), dim)
    }

    /// Current vector matrix, reloaded if another writer bumped `generation`.
    fn dense(&self, c: &Connection) -> rusqlite::Result<Arc<Dense>> {
        let generation: i64 = c
            .query_row("SELECT v FROM meta WHERE k = 'generation'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let cur = self.dense.read().unwrap_or_else(|p| p.into_inner()).clone();
        if cur.generation == generation {
            return Ok(cur);
        }
        let fresh = Arc::new(Dense::load(c, generation)?);
        *self.dense.write().unwrap_or_else(|p| p.into_inner()) = fresh.clone();
        Ok(fresh)
    }

    pub fn search(&self, query: &str, query_embedding: Option<&[f32]>, top_k: usize) -> Retrieval {
        self.try_search(query, query_embedding, top_k).unwrap_or_else(|e| {
            tracing::warn!("retrieval failed, answering without context: {e}");
            Retrieval::default()
        })
    }

    fn try_search(&self, query: &str, query_embedding: Option<&[f32]>, top_k: usize) -> rusqlite::Result<Retrieval> {
        let c = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut lists: Vec<Vec<i64>> = Vec::new();
        let keyword: Vec<i64> = match fts_query(query) {
            Some(m) => {
                let mut st =
                    c.prepare("SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1 ORDER BY rank LIMIT ?2")?;
                let ids = st
                    .query_map(params![m, CANDIDATES as i64], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                ids
            }
            None => Vec::new(),
        };
        lists.push(keyword);
        let mut cos = std::collections::HashMap::new();
        if let Some(q) = query_embedding.and_then(normalize) {
            let dense = self.dense(&c)?;
            if dense.dim > 0 && q.len() != dense.dim {
                tracing::warn!(
                    "query embedding has {} dims, index has {}; keyword-only (reindex after changing research.embeddings_model)",
                    q.len(),
                    dense.dim
                );
            }
            let top = dense.top(&q, CANDIDATES);
            if !top.is_empty() {
                for (id, s) in &top {
                    cos.insert(*id, *s);
                }
                lists.push(top.into_iter().map(|x| x.0).collect());
            }
        }
        let n_lists = lists.len();
        let mut get = c.prepare("SELECT source, text FROM chunks WHERE id = ?1")?;
        let mut hits = Vec::new();
        for (id, score) in rrf(&lists).into_iter().take(top_k) {
            let (source, text): (String, String) = get.query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            hits.push(Hit {
                source,
                text,
                rrf: score,
                cosine: cos.get(&id).copied(),
            });
        }
        Ok(Retrieval {
            top_score: hits.first().map_or(0.0, |h| h.rrf),
            top_cosine: hits.first().and_then(|h| h.cosine),
            lists: n_lists,
            hits,
        })
    }
}

/// CyClaw's score router: RRF ≥ threshold AND (no cosine OR cosine ≥ floor).
/// The RRF threshold assumes two lists; with one list it is halved so a
/// keyword-only index can still pass on a strong match.
pub fn is_confident(r: &Retrieval, min_score: f64, min_semantic: f64) -> bool {
    if r.hits.is_empty() {
        return false;
    }
    let threshold = min_score * (r.lists.max(1) as f64 / 2.0);
    r.top_score >= threshold && r.top_cosine.is_none_or(|c| c >= min_semantic)
}

/// One operator research index: a [`Retriever`] over the SQLite file, the
/// validated config, and an optional loopback embeddings client.
pub struct ResearchStore {
    db: PathBuf,
    retriever: Retriever,
    config: ResearchConfig,
    embedder: Option<EmbedClient>,
}

impl ResearchStore {
    /// Open (creating on first use) the index at `db`. An existing file over
    /// `config.max_db_bytes` is refused rather than grown further.
    pub fn open(db: &Path, config: &ResearchConfig) -> Result<Self> {
        if let Ok(meta) = std::fs::symlink_metadata(db) {
            if meta.len() > config.max_db_bytes {
                return Err(HarnessError::new(
                    RESEARCH_IO,
                    "the research index exceeds research.max_db_bytes; remove it or raise the cap",
                ));
            }
        }
        let index = Index::open(db)?;
        let retriever = index.retriever()?;
        let embedder = if config.embeddings_url.is_empty() {
            None
        } else {
            Some(EmbedClient::new(config)?)
        };
        Ok(Self {
            db: db.to_path_buf(),
            retriever,
            config: config.clone(),
            embedder,
        })
    }

    pub fn config(&self) -> &ResearchConfig {
        &self.config
    }

    pub fn embedder(&self) -> Option<&EmbedClient> {
        self.embedder.as_ref()
    }

    /// `(chunks, embed_model, embed_dim)` for the status route.
    pub fn status(&self) -> Result<(usize, Option<String>, usize)> {
        let (embed_model, embed_dim) = self.retriever.embed_meta();
        Ok((self.retriever.len(), embed_model, embed_dim))
    }

    pub fn search(&self, query: &str, query_embedding: Option<&[f32]>, top_k: usize) -> Retrieval {
        self.retriever.search(query, query_embedding, top_k)
    }

    /// Rebuild the index from the configured corpus. Returns
    /// (files read, chunks, vectors stored). An embeddings failure degrades
    /// the rebuild to keyword-only instead of failing it.
    pub async fn reindex(&self) -> Result<(usize, usize, usize)> {
        let corpus = PathBuf::from(self.config.corpus_dir.clone());
        let chunk_chars = self.config.chunk_chars;
        let chunk_overlap = self.config.chunk_overlap;
        let max_file_bytes = self.config.max_file_bytes;
        let max_total_bytes = self.config.max_total_bytes;
        let (files, pieces) =
            off_worker(move || collect_pieces(&corpus, chunk_chars, chunk_overlap, max_file_bytes, max_total_bytes))
                .await?;
        let mut embeddings: Vec<Option<Vec<f32>>> = vec![None; pieces.len()];
        if let Some(embedder) = &self.embedder {
            for (bi, batch) in pieces.chunks(32).enumerate() {
                let inputs: Vec<String> = batch.iter().map(|p| p.2.clone()).collect();
                match embedder.embed(&inputs).await {
                    Ok(vs) => {
                        for (j, v) in vs.into_iter().enumerate() {
                            embeddings[bi * 32 + j] = Some(v);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("research embedding failed, index will be keyword-only: {e}");
                        break;
                    }
                }
            }
        }
        let rows: Vec<(String, i64, String, Option<Vec<f32>>)> = pieces
            .into_iter()
            .zip(embeddings)
            .map(|((src, ord, text), e)| (src, ord as i64, text, e))
            .collect();
        let chunks = rows.len();
        let db = self.db.clone();
        let embed_model = self.config.embeddings_model.clone();
        let max_db_bytes = self.config.max_db_bytes;
        let stored = off_worker(move || -> Result<usize> {
            let stored = write_index(&db, &rows, &embed_model)?;
            let size = std::fs::symlink_metadata(&db).map(|m| m.len()).unwrap_or(0);
            if size > max_db_bytes {
                return Err(HarnessError::new(
                    RESEARCH_IO,
                    "the research index exceeds research.max_db_bytes after reindex",
                ));
            }
            Ok(stored)
        })
        .await?;
        Ok((files, chunks, stored))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(i: i64, t: &str, e: Option<Vec<f32>>) -> Chunk {
        Chunk {
            id: i,
            source: format!("d{i}.md"),
            text: t.into(),
            embedding: e,
        }
    }

    #[test]
    fn rrf_matches_formula() {
        let f = rrf(&[vec![0, 1], vec![0, 2]]);
        assert_eq!(f[0].0, 0);
        assert!((f[0].1 - 2.0 / 61.0).abs() < 1e-12);
    }

    #[test]
    fn hybrid_search_ranks_and_gates() {
        let r = Retriever::new(vec![
            ch(
                1,
                "veeam backup repository immutability hardened linux",
                Some(vec![1.0, 0.0]),
            ),
            ch(2, "pasta recipe with tomatoes and basil", Some(vec![0.0, 1.0])),
        ]);
        let hit = r.search("veeam immutability", Some(&[0.9, 0.1]), 3);
        assert_eq!(hit.hits[0].source, "d1.md");
        assert_eq!(hit.lists, 2);
        assert!(is_confident(&hit, 0.028, 0.30));
        let miss = r.search("veeam immutability", Some(&[0.0, 1.0]), 3);
        // cosine of the top fused hit is below the floor -> not confident
        assert!(!is_confident(&miss, 0.028, 0.99));
        let kw = r.search("veeam", None, 3);
        assert_eq!(kw.lists, 1);
        assert!(is_confident(&kw, 0.028, 0.30));
        assert!(!is_confident(&r.search("zzz", None, 3), 0.028, 0.30));
    }

    #[test]
    fn chunker_overlaps_and_terminates() {
        let t = "word ".repeat(500);
        let c = chunk_text(&t, 100, 20);
        assert!(c.len() > 20 && c.iter().all(|x| x.len() <= 100));
        assert_eq!(chunk_text("", 100, 20).len(), 0);
    }

    #[test]
    fn fts_query_neutralizes_fts5_syntax() {
        assert_eq!(fts_query("the a"), None);
        let q = fts_query("NEAR(veeam* OR \"x\") col:secret ^start").expect("q");
        // every term is a quoted phrase; no bare operators survive
        for part in q.split(" OR ") {
            assert!(part.starts_with('"') && part.ends_with('"'), "{part}");
        }
        let r = Retriever::new(vec![ch(1, "veeam near start", None)]);
        // must not error out on hostile syntax
        let _ = r.search("NEAR(veeam* OR \"x\") col:secret ^start", None, 3);
    }

    #[test]
    fn bad_vectors_are_dropped_not_stored() {
        assert!(normalize(&[0.0, 0.0]).is_none());
        assert!(normalize(&[f32::NAN, 1.0]).is_none());
        assert!(normalize(&[]).is_none());
        let n = normalize(&[3.0, 4.0]).expect("n");
        assert!((n[0] - 0.6).abs() < 1e-6 && (n[1] - 0.8).abs() < 1e-6);
        let r = Retriever::new(vec![
            ch(1, "alpha", Some(vec![1.0, 0.0])),
            ch(2, "beta", Some(vec![f32::INFINITY, 0.0])),
            ch(3, "gamma", Some(vec![1.0, 0.0, 0.0])),
        ]);
        assert_eq!(r.len(), 3, "all chunks stay keyword-searchable");
        let hit = r.search("zzz", Some(&[1.0, 0.0]), 5);
        assert_eq!(hit.hits.len(), 1, "only the one valid 2-d vector ranks");
        // wrong-dimension query degrades to keyword-only instead of erroring
        assert_eq!(r.search("alpha", Some(&[1.0, 0.0, 0.0]), 5).lists, 1);
    }

    #[test]
    fn exact_scan_matches_brute_force_ordering() {
        let vecs: Vec<Vec<f32>> = (0..40)
            .map(|i| {
                let a = i as f32 * 0.15;
                (0..19).map(|j| (a + j as f32 * 0.37).sin()).collect()
            })
            .collect();
        let chunks = vecs
            .iter()
            .enumerate()
            .map(|(i, v)| ch(i as i64, &format!("doc {i}"), Some(v.clone())))
            .collect();
        let r = Retriever::new(chunks);
        let q = vecs[7].clone();
        let got = r.search("zzz", Some(&q), 5);
        assert_eq!(got.hits[0].source, "d7.md");
        assert!((got.top_cosine.expect("cos") - 1.0).abs() < 1e-5);
        let qn = normalize(&q).expect("q");
        let mut want: Vec<(usize, f32)> = vecs
            .iter()
            .enumerate()
            .map(|(i, v)| (i, dot(&normalize(v).expect("v"), &qn)))
            .collect();
        want.sort_by(|a, b| b.1.total_cmp(&a.1));
        let got_src: Vec<_> = got.hits.iter().map(|h| h.source.clone()).collect();
        let want_src: Vec<_> = want.iter().take(5).map(|x| format!("d{}.md", x.0)).collect();
        assert_eq!(got_src, want_src);
    }

    #[test]
    fn running_retriever_sees_a_rebuild_from_another_connection() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = dir.path().join("idx/rag.sqlite3");
        let idx = Index::open(&db).expect("open");
        let r = idx.retriever().expect("r");
        assert!(r.is_empty());
        let mut w = Connection::open(&db).expect("w");
        write_all(
            &mut w,
            &[("n.md".into(), 0, "immutable repository".into(), Some(vec![1.0, 0.0]))],
        )
        .expect("write");
        assert_eq!(r.len(), 1);
        let hit = r.search("immutable", Some(&[1.0, 0.0]), 3);
        assert_eq!(hit.lists, 2, "vector matrix reloaded after generation bump");
        // reader is query-only
        assert!(r.conn.lock().expect("l").execute("DELETE FROM chunks", []).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(&db).expect("m").permissions().mode() & 0o777;
            assert_eq!(m, 0o600);
        }
    }

    #[test]
    fn old_schema_is_dropped_and_rebuilt() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = dir.path().join("rag.sqlite3");
        let c = Connection::open(&db).expect("c");
        c.execute_batch(
            "CREATE TABLE chunks(id INTEGER PRIMARY KEY, source TEXT, ord INTEGER, text TEXT, embedding BLOB);
             INSERT INTO chunks VALUES (1, 's', 0, 'old', NULL);
             CREATE TABLE meta(k TEXT PRIMARY KEY, v TEXT NOT NULL);
             INSERT INTO meta VALUES ('embed_model', 'x');",
        )
        .expect("old");
        drop(c);
        let r = Index::open(&db).expect("open").retriever().expect("r");
        assert!(r.is_empty(), "v1 rows are discarded; reindex required");
    }

    #[test]
    fn corpus_collection_never_follows_symlinks_and_caps_bytes() {
        let dir = tempfile::tempdir().expect("tmp");
        let corpus = dir.path().join("corpus");
        std::fs::create_dir_all(&corpus).expect("corpus");
        std::fs::write(corpus.join("a.md"), "alpha beta gamma").expect("a");
        std::fs::write(corpus.join("b.txt"), "delta epsilon").expect("b");
        std::fs::write(corpus.join("c.exe"), "not indexed").expect("c");
        let outside = dir.path().join("outside.md");
        std::fs::write(&outside, "escaped secret").expect("outside");
        // Symlink creation needs a privilege on some Windows hosts; when the
        // link cannot be created the no-follow assertions degrade to the
        // extension filter, which the remaining assertions still cover.
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, corpus.join("link.md")).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&outside, corpus.join("link.md")).is_ok();
        let (files, pieces) = collect_pieces(&corpus, 900, 120, 1_048_576, 67_108_864).expect("collect");
        if linked {
            assert_eq!(files, 2);
            assert!(!pieces.iter().any(|(src, _, _)| src.contains("link")));
        }
        assert!(!pieces.iter().any(|(_, _, text)| text.contains("escaped")));
        // max_total_bytes stops collection
        let (files, _) = collect_pieces(&corpus, 900, 120, 1_048_576, 1).expect("collect");
        assert_eq!(files, 1);
        // max_file_bytes skips oversized files
        let (files, _) = collect_pieces(&corpus, 900, 120, 4, 67_108_864).expect("collect");
        assert_eq!(files, 0);
        // a missing corpus is a corpus error, not an IO panic
        let err =
            collect_pieces(&dir.path().join("missing"), 900, 120, 1_048_576, 67_108_864).expect_err("missing corpus");
        assert!(err.is(RESEARCH_NO_CORPUS));
    }
}
