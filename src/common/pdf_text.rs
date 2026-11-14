//! Text-only PDF extraction (issue 813), shared by chat attachments, the notes
//! corpus and exact web reads, and meant for any later PDF source.
//!
//! [`extract`] is the only entry point callers use. It parses in the hidden
//! `pdf-text-worker` child of this executable, under a hard kill deadline,
//! because a parser call cannot be cancelled once it runs on a thread. The
//! child limits its own heap (every OS, `common::alloc_cap`), address space
//! (Linux), CPU time and file writes before it reads a byte. Results are cached in memory by content digest; a cache hit
//! never replaces the caller's owner and digest checks, which run first.
//!
//! Only page content streams are read. Images, form XObjects, annotations,
//! links, actions, JavaScript, forms and embedded files are never decoded,
//! followed or executed. No OCR. Encrypted PDFs are refused, including ones
//! that open with an empty password. Text order follows the content stream,
//! which can differ from the visual order of columns and tables.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::common::errors::{HarnessError, Result};

pub const MIME: &str = "application/pdf";
/// Largest PDF any source may hand the extractor (the attachment file limit).
pub const MAX_INPUT_BYTES: usize = 15 * 1024 * 1024;
/// Documents with more pages are refused, not read partially.
pub const MAX_PAGES: usize = 200;
/// Ceiling on the normalized text of one document, in UTF-8 bytes.
pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// One document's parse, including worker start-up.
pub const FILE_DEADLINE: Duration = Duration::from_secs(5);
/// Bumped whenever the rendered text changes, so cached results never mix versions.
const EXTRACTOR_VERSION: u32 = 1;
/// Any one stream decoded while loading (object and cross-reference streams).
const MAX_LOAD_STREAM_BYTES: usize = 8 * 1024 * 1024;
/// Decoded content streams of one page, and of the whole document.
const MAX_PAGE_CONTENT_BYTES: usize = 8 * 1024 * 1024;
const MAX_DOC_CONTENT_BYTES: usize = 32 * 1024 * 1024;
/// Parsed objects; real documents of the supported size stay far below it.
const MAX_OBJECTS: usize = 400_000;
/// Linux address-space ceiling the worker sets on itself.
#[cfg(target_os = "linux")]
const WORKER_ADDRESS_SPACE_BYTES: u64 = 1024 * 1024 * 1024;
/// Heap ceiling armed in the worker on every OS (`common::alloc_cap`); below
/// the Linux address-space cap so it is the bound that normally trips.
const WORKER_HEAP_BYTES: usize = 768 * 1024 * 1024;
/// Concurrent workers across uploads, previews, chat turns, loops and web reads.
const MAX_WORKERS: usize = 2;
/// Cached extractions: entries and total text bytes held in memory.
const CACHE_ENTRIES: usize = 128;
const CACHE_BYTES: usize = 48 * 1024 * 1024;
/// Pages listed by number in the empty-page note before it summarizes.
const LISTED_EMPTY_PAGES: usize = 24;

/// Stable refusal codes. Messages are fixed text: parser diagnostics and
/// document strings never reach an error.
const CODES: &[(&str, &str)] = &[
    ("PDF_MALFORMED", "PDF structure could not be read"),
    ("PDF_ENCRYPTED", "encrypted PDFs are not supported"),
    (
        "PDF_NO_TEXT",
        "no supported text was found in this PDF (no OCR is performed)",
    ),
    (
        "PDF_LIMIT",
        "PDF exceeds a supported size, page or decoded-content limit",
    ),
    ("PDF_TIMEOUT", "PDF text extraction exceeded its deadline"),
    ("PDF_BUSY", "PDF text extraction is busy; retry shortly"),
    ("PDF_WORKER_FAILED", "PDF text extraction failed"),
];

pub fn refusal(code: &str) -> HarnessError {
    let (code, message) = CODES
        .iter()
        .find(|(c, _)| *c == code)
        .copied()
        .unwrap_or(CODES[CODES.len() - 1]);
    HarnessError::new(code, message)
}

/// True when `data` starts with the PDF header at offset zero.
pub fn looks_like_pdf(data: &[u8]) -> bool {
    data.starts_with(b"%PDF-")
}

/// Extracts prompt-ready text from untrusted PDF bytes, refusing with one of
/// the `PDF_*` codes. `deadline` bounds queueing plus the parse; it is cut to
/// [`FILE_DEADLINE`] per document. Blocking: call from a blocking context.
pub fn extract(data: &[u8], deadline: Instant) -> Result<Arc<str>> {
    if data.len() > MAX_INPUT_BYTES {
        return Err(refusal("PDF_LIMIT").detail("limit_bytes", MAX_INPUT_BYTES as u64));
    }
    if !looks_like_pdf(data) {
        return Err(refusal("PDF_MALFORMED"));
    }
    let key = format!("{EXTRACTOR_VERSION}:{}", crate::common::sha256_bytes_hex(data));
    if let Some(hit) = cache().get(&key) {
        return Ok(hit);
    }
    let deadline = deadline.min(Instant::now() + FILE_DEADLINE);
    let _slot = Slots::global().acquire(deadline)?;
    // Another caller may have finished the same bytes while this one queued.
    if let Some(hit) = cache().get(&key) {
        return Ok(hit);
    }
    let text: Arc<str> = Arc::from(run_isolated(data, deadline)?);
    cache().put(key, text.clone());
    Ok(text)
}

/// Drops cached text for these digests (owner cleanup). Another owner with the
/// same bytes simply extracts again.
pub fn forget(sha256_hex: &[String]) {
    let keys: Vec<String> = sha256_hex.iter().map(|s| format!("{EXTRACTOR_VERSION}:{s}")).collect();
    cache().remove(&keys);
}

// ---------------------------------------------------------------------------
// Isolation

#[cfg(any(test, feature = "test-support"))]
static TEST_WORKER_EXE: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Test hook: the executable whose `pdf-text-worker` subcommand integration
/// tests use. Unit tests that never set it parse in-process instead.
#[cfg(any(test, feature = "test-support"))]
pub fn set_test_worker_exe(path: std::path::PathBuf) {
    let _ = TEST_WORKER_EXE.set(path);
}

/// This binary's path, resolved once. Linux reports a replaced executable as
/// `<path> (deleted)`, which cannot be spawned, so a lookup per read would
/// refuse every PDF after an in-place upgrade until the server restarts.
#[cfg(not(any(test, feature = "test-support")))]
fn worker_exe() -> Option<&'static std::path::PathBuf> {
    static EXE: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    EXE.get_or_init(|| std::env::current_exe().ok()).as_ref()
}

/// Resolves the worker path at server startup, as the shim does for its child,
/// before the binary on disk can be replaced under a running server.
pub fn resolve_worker_exe() {
    #[cfg(not(any(test, feature = "test-support")))]
    let _ = worker_exe();
}

fn run_isolated(data: &[u8], deadline: Instant) -> Result<String> {
    #[cfg(any(test, feature = "test-support"))]
    let exe = match TEST_WORKER_EXE.get() {
        Some(exe) => exe.clone(),
        None => return extract_in_process(data, deadline).map_err(PdfFailure::into_error),
    };
    #[cfg(not(any(test, feature = "test-support")))]
    let exe = worker_exe().ok_or_else(|| refusal("PDF_WORKER_FAILED"))?.clone();
    let timeout = deadline.saturating_duration_since(Instant::now());
    if timeout.is_zero() {
        return Err(refusal("PDF_TIMEOUT"));
    }
    let argv = [exe.to_string_lossy().into_owned(), "pdf-text-worker".to_string()];
    // Nothing inherited: no credentials, proxies or locale reach the parser.
    let env = BTreeMap::new();
    let output = crate::common::process::run(crate::common::process::RunSpec {
        argv: &argv,
        cwd: None,
        env: Some(&env),
        timeout,
        stdin: Some(data),
    })
    .map_err(|e| match e {
        crate::common::process::ProcessError::Timeout { .. } => refusal("PDF_TIMEOUT"),
        _ => refusal("PDF_WORKER_FAILED"),
    })?;
    let reply: WorkerReply = serde_json::from_str(output.stdout.trim()).map_err(|_| match output.signal {
        // SIGXCPU or an allocation failure under the address-space limit.
        Some(_) => refusal("PDF_LIMIT"),
        None => refusal("PDF_WORKER_FAILED"),
    })?;
    match (output.status, reply) {
        (Some(0), WorkerReply::Text { text }) if text.len() <= MAX_OUTPUT_BYTES => Ok(text),
        (_, WorkerReply::Refused { code }) => Err(refusal(&code)),
        _ => Err(refusal("PDF_WORKER_FAILED")),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
enum WorkerReply {
    Text { text: String },
    Refused { code: String },
}

/// `cgagentharness pdf-text-worker`: PDF bytes on stdin, one JSON reply on
/// stdout. Exit 0 with text, 2 refused, 3 for an unusable environment.
pub fn worker_main() -> u8 {
    use std::io::{Read, Write};
    // First, so the parse below is counted. Exceeding it aborts this child.
    crate::common::alloc_cap::arm(WORKER_HEAP_BYTES);
    if limit_self().is_err() {
        return 3;
    }
    let started = Instant::now();
    let mut data = Vec::new();
    let read = std::io::stdin()
        .lock()
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut data);
    let reply = match read {
        Err(_) => WorkerReply::Refused {
            code: "PDF_WORKER_FAILED".into(),
        },
        Ok(_) if data.len() > MAX_INPUT_BYTES => WorkerReply::Refused {
            code: "PDF_LIMIT".into(),
        },
        // The parent's kill deadline is the real bound; this one ends clean.
        Ok(_) => match extract_in_process(&data, started + FILE_DEADLINE) {
            Ok(text) => WorkerReply::Text { text },
            Err(failure) => WorkerReply::Refused {
                code: failure.code().into(),
            },
        },
    };
    let code = if matches!(reply, WorkerReply::Text { .. }) {
        0
    } else {
        2
    };
    let Ok(json) = serde_json::to_string(&reply) else {
        return 3;
    };
    let mut out = std::io::stdout().lock();
    if out.write_all(json.as_bytes()).and_then(|_| out.flush()).is_err() {
        return 3;
    }
    code
}

#[cfg(unix)]
fn limit_self() -> std::io::Result<()> {
    fn set(resource: libc::c_int, value: u64) -> std::io::Result<()> {
        let limit = libc::rlimit {
            rlim_cur: value as libc::rlim_t,
            rlim_max: value as libc::rlim_t,
        };
        // SAFETY: setrlimit reads one fully initialized struct; it only lowers this process's limits.
        if unsafe { libc::setrlimit(resource as _, &limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    // CPU seconds past the parent's deadline end the process even if it is never killed.
    set(libc::RLIMIT_CPU as _, FILE_DEADLINE.as_secs() + 2)?;
    // No regular-file writes; pipes are unaffected.
    set(libc::RLIMIT_FSIZE as _, 0)?;
    set(libc::RLIMIT_CORE as _, 0)?;
    #[cfg(target_os = "linux")]
    set(libc::RLIMIT_AS as _, WORKER_ADDRESS_SPACE_BYTES)?;
    Ok(())
}

#[cfg(not(unix))]
fn limit_self() -> std::io::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Admission and cache

struct Slots {
    used: Mutex<usize>,
    freed: Condvar,
}

struct SlotGuard(&'static Slots);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        *self.0.used.lock().unwrap_or_else(|p| p.into_inner()) -= 1;
        self.0.freed.notify_one();
    }
}

impl Slots {
    fn global() -> &'static Slots {
        static SLOTS: OnceLock<Slots> = OnceLock::new();
        SLOTS.get_or_init(|| Slots {
            used: Mutex::new(0),
            freed: Condvar::new(),
        })
    }

    fn acquire(&'static self, deadline: Instant) -> Result<SlotGuard> {
        let mut used = self.used.lock().unwrap_or_else(|p| p.into_inner());
        while *used >= MAX_WORKERS {
            let wait = deadline.saturating_duration_since(Instant::now());
            if wait.is_zero() {
                return Err(refusal("PDF_BUSY"));
            }
            used = self.freed.wait_timeout(used, wait).unwrap_or_else(|p| p.into_inner()).0;
        }
        *used += 1;
        Ok(SlotGuard(self))
    }
}

#[derive(Default)]
struct Cache {
    inner: Mutex<CacheInner>,
}

#[derive(Default)]
struct CacheInner {
    map: HashMap<String, Arc<str>>,
    order: VecDeque<String>,
    bytes: usize,
}

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Cache::default)
}

impl Cache {
    fn get(&self, key: &str) -> Option<Arc<str>> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let hit = inner.map.get(key).cloned()?;
        if let Some(at) = inner.order.iter().position(|k| k == key) {
            if let Some(k) = inner.order.remove(at) {
                inner.order.push_back(k);
            }
        }
        Some(hit)
    }

    fn put(&self, key: String, text: Arc<str>) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.map.contains_key(&key) {
            return;
        }
        inner.bytes += text.len();
        inner.map.insert(key.clone(), text);
        inner.order.push_back(key);
        while inner.map.len() > CACHE_ENTRIES || inner.bytes > CACHE_BYTES {
            let Some(old) = inner.order.pop_front() else { break };
            if let Some(text) = inner.map.remove(&old) {
                inner.bytes -= text.len();
            }
        }
    }

    fn remove(&self, keys: &[String]) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        for key in keys {
            if let Some(text) = inner.map.remove(key) {
                inner.bytes -= text.len();
                inner.order.retain(|k| k != key);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing (runs in the worker; in-process only for unit tests)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PdfFailure {
    Malformed,
    Encrypted,
    NoText,
    Limit,
    Timeout,
}

impl PdfFailure {
    fn code(self) -> &'static str {
        match self {
            PdfFailure::Malformed => "PDF_MALFORMED",
            PdfFailure::Encrypted => "PDF_ENCRYPTED",
            PdfFailure::NoText => "PDF_NO_TEXT",
            PdfFailure::Limit => "PDF_LIMIT",
            PdfFailure::Timeout => "PDF_TIMEOUT",
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn into_error(self) -> HarnessError {
        refusal(self.code())
    }
}

fn from_lopdf(e: &lopdf::Error) -> PdfFailure {
    match e {
        lopdf::Error::Decompress(lopdf::DecompressError::MemoryLimitExceeded { .. }) => PdfFailure::Limit,
        lopdf::Error::Decryption(_) | lopdf::Error::InvalidPassword => PdfFailure::Encrypted,
        _ => PdfFailure::Malformed,
    }
}

fn extract_in_process(data: &[u8], deadline: Instant) -> std::result::Result<String, PdfFailure> {
    if data.len() > MAX_INPUT_BYTES {
        return Err(PdfFailure::Limit);
    }
    if !looks_like_pdf(data) {
        return Err(PdfFailure::Malformed);
    }
    // No load filter: lopdf rebuilds a damaged cross-reference table only for
    // unfiltered loads, and image or attachment streams are never decoded here.
    let options = lopdf::LoadOptions {
        max_decompressed_size: Some(MAX_LOAD_STREAM_BYTES),
        ..Default::default()
    };
    let doc = lopdf::Document::load_mem_with_options(data, options).map_err(|e| from_lopdf(&e))?;
    // lopdf opens an empty-password file silently; the original state decides.
    if doc.was_encrypted() || doc.is_encrypted() || doc.trailer.has(b"Encrypt") {
        return Err(PdfFailure::Encrypted);
    }
    if doc.objects.len() > MAX_OBJECTS {
        return Err(PdfFailure::Limit);
    }
    let pages = doc.get_pages();
    if pages.is_empty() {
        return Err(PdfFailure::Malformed);
    }
    if pages.len() > MAX_PAGES {
        return Err(PdfFailure::Limit);
    }
    let mut budget = MAX_DOC_CONTENT_BYTES;
    let mut raw_pages = Vec::with_capacity(pages.len());
    for page_id in pages.values() {
        if Instant::now() >= deadline {
            return Err(PdfFailure::Timeout);
        }
        raw_pages.push(page_text(&doc, *page_id, &mut budget)?);
    }
    render(raw_pages, deadline)
}

/// Text shown on one page, in content-stream order. A page whose fonts or
/// content cannot be decoded yields no text rather than failing the document.
fn page_text(
    doc: &lopdf::Document,
    page_id: lopdf::ObjectId,
    budget: &mut usize,
) -> std::result::Result<String, PdfFailure> {
    let limit = MAX_PAGE_CONTENT_BYTES.min(*budget);
    let content = match doc.get_page_content_with_limit(page_id, limit) {
        Ok(content) => content,
        Err(e) if from_lopdf(&e) == PdfFailure::Limit => return Err(PdfFailure::Limit),
        Err(_) => return Ok(String::new()),
    };
    *budget = budget.saturating_sub(content.len());
    let Ok(fonts) = doc.get_page_fonts(page_id) else {
        return Ok(String::new());
    };
    let mut encodings = BTreeMap::new();
    for (name, font) in fonts {
        if let Some(codec) = builtin_symbolic(font) {
            encodings.insert(name, codec);
            continue;
        }
        match font.get_font_encoding_with_limit(doc, MAX_PAGE_CONTENT_BYTES) {
            Ok(encoding) => {
                encodings.insert(name, FontCodec::Mapped(encoding));
            }
            Err(e) if from_lopdf(&e) == PdfFailure::Limit => return Err(PdfFailure::Limit),
            Err(_) => {}
        }
    }
    let Ok(content) = lopdf::content::Content::decode(&content) else {
        return Ok(String::new());
    };
    let mut walker = TextWalker::default();
    for op in &content.operations {
        walker.apply(&op.operator, &op.operands, &encodings);
        if walker.out.len() > MAX_OUTPUT_BYTES * 4 {
            return Err(PdfFailure::Limit);
        }
    }
    Ok(walker.out)
}

/// How a font's string bytes become text.
enum FontCodec<'a> {
    /// The font's `/Encoding` or `/ToUnicode`, as lopdf reads it.
    Mapped(lopdf::Encoding<'a>),
    /// Built-in encoding of the standard Symbol font (Greek, math).
    Symbol,
    /// Built-in encoding of ZapfDingbats (check marks, bullets, boxes).
    Dingbats,
}

/// Symbol and ZapfDingbats carry their own built-in encodings. With no
/// `/Encoding` or `/ToUnicode`, lopdf would read them as StandardEncoding:
/// Symbol's "a" (alpha) as a Latin "a", and a Dingbats check mark as "4".
fn builtin_symbolic<'a>(font: &lopdf::Dictionary) -> Option<FontCodec<'a>> {
    if font.has(b"Encoding") || font.has(b"ToUnicode") {
        return None;
    }
    let base = font.get(b"BaseFont").and_then(lopdf::Object::as_name).ok()?;
    // A subset font is named `ABCDEF+Name`.
    let base = base.split(|b| *b == b'+').next_back().unwrap_or(base);
    if base.starts_with(b"Symbol") {
        Some(FontCodec::Symbol)
    } else if base.starts_with(b"ZapfDingbats") || base.starts_with(b"Dingbats") {
        Some(FontCodec::Dingbats)
    } else {
        None
    }
}

/// Adobe Symbol built-in encoding. Unlisted printable ASCII is unchanged;
/// bracket and integral extension pieces have no text and are dropped.
fn symbol_char(byte: u8) -> Option<char> {
    const UPPER: &str = "ΑΒΧΔΕΦΓΗΙϑΚΛΜΝΟΠΘΡΣΤΥςΩΞΨΖ";
    const LOWER: &str = "αβχδεφγηιϕκλμνοπθρστυϖωξψζ";
    // 0xA1..=0xE5: ϒ′≤⁄∞ƒ♣♦♥♠↔←↑→↓°±″≥×∝∂•÷≠≡≈…⏐⎯↵ℵℑℜ℘⊗⊕∅∩∪⊃⊇⊄⊂⊆∈∉∠∇®©™∏√⋅¬∧∨⇔⇐⇑⇒⇓◊〈®©™∑
    const HIGH: &str = "ϒ′≤⁄∞ƒ♣♦♥♠↔←↑→↓°±″≥×∝∂•÷≠≡≈…⏐⎯↵ℵℑℜ℘⊗⊕∅∩∪⊃⊇⊄⊂⊆∈∉∠∇®©™∏√⋅¬∧∨⇔⇐⇑⇒⇓◊〈®©™∑";
    match byte {
        b'A'..=b'Z' => UPPER.chars().nth(usize::from(byte - b'A')),
        b'a'..=b'z' => LOWER.chars().nth(usize::from(byte - b'a')),
        b'"' => Some('∀'),
        b'$' => Some('∃'),
        b'\'' => Some('∋'),
        b'*' => Some('∗'),
        b'-' => Some('−'),
        b'@' => Some('≅'),
        b'\\' => Some('∴'),
        b'^' => Some('⊥'),
        b'`' => Some('‾'),
        b'~' => Some('∼'),
        0x20..=0x7E => Some(char::from(byte)),
        0xA0 => Some('€'),
        0xA1..=0xE5 => HIGH.chars().nth(usize::from(byte - 0xA1)),
        0xF1 => Some('〉'),
        0xF2 => Some('∫'),
        _ => None,
    }
}

/// ZapfDingbats: the marks forms and lists use; any other glyph is a bullet,
/// never the Latin letter or digit its code happens to share.
fn dingbat_char(byte: u8) -> Option<char> {
    match byte {
        b' ' => Some(' '),
        b'3' => Some('✓'),
        b'4' => Some('✔'),
        b'5' => Some('✕'),
        b'6' => Some('✖'),
        b'7' => Some('✗'),
        b'8' => Some('✘'),
        b'H' => Some('★'),
        b'l' => Some('●'),
        b'm' => Some('❍'),
        b'n' => Some('■'),
        b'o' => Some('❏'),
        b'p' => Some('❐'),
        b'q' => Some('❑'),
        b'r' => Some('❒'),
        b'u' => Some('◆'),
        0x21..=0xFE => Some('•'),
        _ => None,
    }
}

#[derive(Default)]
struct TextWalker<'a> {
    out: String,
    encoding: Option<&'a FontCodec<'a>>,
    line_y: Option<f32>,
    /// `Tf` size and the text matrix's vertical scale: a line break is a
    /// vertical move of over half an em, so superscripts stay on their line.
    font_size: f32,
    scale: Option<f32>,
    /// Font and size saved by `q`, restored by `Q`; nesting past
    /// [`MAX_STATE_DEPTH`] is not tracked.
    saved: Vec<(Option<&'a FontCodec<'a>>, f32)>,
    /// A `T*`-style move to the next line, written only once the next text
    /// shows where it lands: a `Tm` back onto the baseline of the last shown
    /// text (the next cell of a table row) is a space, anything else a newline.
    pending_break: bool,
    shown_y: Option<f32>,
}

/// The PDF graphics-state nesting limit implementations are expected to support.
const MAX_STATE_DEPTH: usize = 28;

fn number(o: &lopdf::Object) -> Option<f32> {
    match o {
        lopdf::Object::Integer(i) => Some(*i as f32),
        lopdf::Object::Real(r) => Some(*r),
        _ => None,
    }
}

impl<'a> TextWalker<'a> {
    /// Half the current em in user space; unknown sizes break on any move.
    fn half_em(&self) -> f32 {
        (self.font_size * self.scale.unwrap_or(1.0) * 0.5).max(0.01)
    }

    fn newline(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn space(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with([' ', '\n']) {
            self.out.push(' ');
        }
    }

    fn show(&mut self, operand: Option<&lopdf::Object>) {
        let (Some(encoding), Some(lopdf::Object::String(bytes, _))) = (self.encoding, operand) else {
            return;
        };
        if std::mem::take(&mut self.pending_break) {
            self.newline();
        }
        self.shown_y = self.line_y;
        match encoding {
            FontCodec::Mapped(encoding) => {
                if let Ok(text) = lopdf::Document::decode_text(encoding, bytes) {
                    self.out.push_str(&text);
                }
            }
            FontCodec::Symbol => self.out.extend(bytes.iter().filter_map(|b| symbol_char(*b))),
            FontCodec::Dingbats => self.out.extend(bytes.iter().filter_map(|b| dingbat_char(*b))),
        }
    }

    fn apply(&mut self, operator: &str, operands: &[lopdf::Object], encodings: &'a BTreeMap<Vec<u8>, FontCodec<'a>>) {
        match operator {
            "Tf" => {
                self.encoding = operands
                    .first()
                    .and_then(|o| o.as_name().ok())
                    .and_then(|name| encodings.get(name));
                self.font_size = operands.get(1).and_then(number).map(f32::abs).unwrap_or(0.0);
            }
            "Tj" => self.show(operands.first()),
            "'" => {
                self.pending_break = true;
                self.show(operands.first());
            }
            "\"" => {
                self.pending_break = true;
                self.show(operands.get(2));
            }
            "TJ" => {
                let Some(lopdf::Object::Array(items)) = operands.first() else {
                    return;
                };
                for item in items {
                    match number(item) {
                        // Thousandths of an em. Measured on pdfTeX output, kerning
                        // stays within -50..100 and justified word gaps within
                        // -400..-150, so -120 separates them at any font size.
                        Some(gap) if gap < -120.0 => self.space(),
                        Some(_) => {}
                        None => self.show(Some(item)),
                    }
                }
            }
            "Td" | "TD" => {
                if std::mem::take(&mut self.pending_break) {
                    self.newline();
                }
                let tx = operands.first().and_then(number).unwrap_or(0.0);
                let dy = operands.get(1).and_then(number).unwrap_or(0.0) * self.scale.unwrap_or(1.0);
                self.line_y = self.line_y.map(|y| y + dy);
                if dy.abs() > self.half_em() {
                    self.newline();
                } else if tx.abs() > 0.01 || dy.abs() > 0.01 {
                    self.space();
                }
            }
            "Tm" => {
                let y = operands.get(5).and_then(number);
                self.scale = operands.get(3).and_then(number).map(f32::abs).filter(|d| *d > 0.0);
                if std::mem::take(&mut self.pending_break) {
                    match (self.shown_y, y) {
                        (Some(shown), Some(y)) if (shown - y).abs() <= self.half_em() => self.space(),
                        _ => self.newline(),
                    }
                    self.line_y = y;
                    return;
                }
                match (self.line_y, y) {
                    (Some(prev), Some(y)) if (prev - y).abs() > self.half_em() => self.newline(),
                    (Some(_), Some(_)) => self.space(),
                    _ => self.newline(),
                }
                self.line_y = y;
            }
            "T*" => self.pending_break = true,
            "q" if self.saved.len() < MAX_STATE_DEPTH => self.saved.push((self.encoding, self.font_size)),
            "Q" => {
                if let Some((encoding, size)) = self.saved.pop() {
                    self.encoding = encoding;
                    self.font_size = size;
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering: compact, page-addressed text for small-context local models

/// Characters that carry no text: controls, soft hyphens, zero-width marks.
fn invisible(c: char) -> bool {
    (c.is_control() && c != '\n') || matches!(c, '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{2060}' | '\u{FEFF}')
}

/// Mostly replacement or private-use characters: a font without a usable
/// Unicode mapping. Shown as no text rather than as noise.
fn undecodable(text: &str) -> bool {
    let (mut bad, mut total) = (0usize, 0usize);
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        total += 1;
        if c == '\u{FFFD}' || ('\u{E000}'..='\u{F8FF}').contains(&c) {
            bad += 1;
        }
    }
    total > 0 && bad * 3 > total
}

/// NFKC (ligatures such as `ﬁ` become `fi`, so retrieval matches the words),
/// trimmed single-spaced lines, hyphenated line breaks joined.
fn normalize_page(raw: &str) -> Vec<String> {
    let folded: String = raw.nfkc().filter(|c| !invisible(*c)).collect();
    let mut lines: Vec<String> = Vec::new();
    for line in folded.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            if lines.last().is_some_and(|l| !l.is_empty()) {
                lines.push(String::new());
            }
            continue;
        }
        if let Some(prev) = lines.last_mut() {
            let joins = prev.len() > 1
                && prev.ends_with('-')
                && prev[..prev.len() - 1].chars().last().is_some_and(char::is_alphabetic)
                && line.chars().next().is_some_and(char::is_lowercase);
            if joins {
                prev.pop();
                prev.push_str(&line);
                continue;
            }
        }
        lines.push(line);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// Key for a running header or footer: case, spacing and digit values
/// ignored, so "Page 3 of 9" and "Page 4 of 9" share a key. A shared key alone
/// is not enough to remove a line; see [`page_numbered`].
/// A run of digits is one `#`, so page 9 and page 10 share a key; whether
/// the numbers themselves make the line furniture is `page_numbered`'s call.
fn furniture_key(line: &str) -> String {
    let mut key = String::new();
    for c in line.chars().filter(|c| !c.is_whitespace()) {
        if c.is_ascii_digit() {
            if !key.ends_with('#') {
                key.push('#');
            }
        } else {
            key.push(c.to_ascii_lowercase());
        }
    }
    key
}

/// An edge line's page index and its numbers.
type EdgeLine = (usize, Vec<Option<i64>>);

/// Every number in the line, in order; an overlong run counts as distinct.
fn numbers(line: &str) -> Vec<Option<i64>> {
    line.split(|c: char| !c.is_ascii_digit())
        .filter(|run| !run.is_empty())
        .map(|run| run.parse::<i64>().ok())
        .collect()
}

/// Lines sharing a key are furniture only when each number in them is either
/// the same on every page or moves in step with the page index (a page
/// number). "Revenue: $100" .. "$400" on pages 1..4 is content and stays.
fn page_numbered(lines: &[EdgeLine]) -> bool {
    let Some((_, first)) = lines.first() else {
        return false;
    };
    (0..first.len()).all(|at| {
        let value = |(_, nums): &EdgeLine| nums.get(at).copied().flatten();
        let constant = lines.iter().all(|l| value(l).is_some() && value(l) == value(&lines[0]));
        let offset = |l: &EdgeLine| value(l).map(|v| v - l.0 as i64);
        let paged = lines
            .iter()
            .all(|l| offset(l).is_some() && offset(l) == offset(&lines[0]));
        constant || paged
    })
}

/// Removes lines that repeat at the top or bottom of most pages (running
/// headers, footers, page numbers): they cost context and skew ranking.
/// A second pass finds a page number drawn just inside a running header or
/// footer once that is gone. It runs only if the first removed something and
/// removes only bare page numbers, never a line of prose that now sits at an edge.
fn strip_furniture(pages: &mut [Vec<String>]) {
    if strip_furniture_once(pages, false) {
        strip_furniture_once(pages, true);
    }
}

/// `3`, `- 3 -`, `Page 3`, `p. 3`, `3 of 40`, `3 | 40`.
fn bare_page_number(line: &str) -> bool {
    let lower = line.trim().to_lowercase();
    let rest = lower
        .split_whitespace()
        .filter(|w| !matches!(*w, "page" | "pg" | "pg." | "p." | "of"))
        .collect::<String>();
    lower.len() <= 24
        && rest.chars().any(|c| c.is_ascii_digit())
        && rest.chars().all(|c| c.is_ascii_digit() || "-–—|/.()[]".contains(c))
}

fn strip_furniture_once(pages: &mut [Vec<String>], only_bare: bool) -> bool {
    let edges = |page: &Vec<String>| -> Vec<usize> {
        let first = page.iter().position(|l| !l.is_empty());
        let last = page.iter().rposition(|l| !l.is_empty());
        let mut at: Vec<usize> = first.into_iter().chain(last).collect();
        at.dedup();
        at
    };
    let with_text = pages.iter().filter(|p| !p.is_empty()).count();
    if with_text < 4 {
        return false;
    }
    let mut groups: HashMap<String, Vec<EdgeLine>> = HashMap::new();
    for (index, page) in pages.iter().enumerate() {
        let mut seen = std::collections::HashSet::new();
        for at in edges(page) {
            let key = furniture_key(&page[at]);
            if seen.insert(key.clone()) {
                groups.entry(key).or_default().push((index, numbers(&page[at])));
            }
        }
    }
    let repeated: std::collections::HashSet<String> = groups
        .into_iter()
        .filter(|(key, lines)| {
            !key.is_empty() && lines.len() * 2 >= with_text && lines.len() >= 3 && page_numbered(lines)
        })
        .map(|(key, _)| key)
        .collect();
    if repeated.is_empty() {
        return false;
    }
    for page in pages.iter_mut() {
        for at in edges(page).into_iter().rev() {
            if repeated.contains(&furniture_key(&page[at])) && (!only_bare || bare_page_number(&page[at])) {
                page.remove(at);
            }
        }
        while page.first().is_some_and(String::is_empty) {
            page.remove(0);
        }
        while page.last().is_some_and(String::is_empty) {
            page.pop();
        }
    }
    true
}

fn page_list(pages: &[usize]) -> String {
    let mut shown: Vec<String> = pages.iter().take(LISTED_EMPTY_PAGES).map(usize::to_string).collect();
    if pages.len() > LISTED_EMPTY_PAGES {
        shown.push(format!("and {} more", pages.len() - LISTED_EMPTY_PAGES));
    }
    shown.join(", ")
}

/// One note line, then `# Page N` blocks. The passage index keeps each chunk's
/// last `# ` heading, so retrieved passages carry their page number.
fn render(raw_pages: Vec<String>, deadline: Instant) -> std::result::Result<String, PdfFailure> {
    let total = raw_pages.len();
    let mut pages: Vec<Vec<String>> = Vec::with_capacity(total);
    for raw in &raw_pages {
        if Instant::now() >= deadline {
            return Err(PdfFailure::Timeout);
        }
        let lines = normalize_page(raw);
        pages.push(if undecodable(&lines.join("\n")) {
            Vec::new()
        } else {
            lines
        });
    }
    strip_furniture(&mut pages);
    let empty: Vec<usize> = pages
        .iter()
        .enumerate()
        .filter(|(_, p)| p.is_empty())
        .map(|(i, _)| i + 1)
        .collect();
    if empty.len() == total {
        return Err(PdfFailure::NoText);
    }
    let plural = if total == 1 { "page" } else { "pages" };
    let mut out = format!("[PDF text: {total} {plural}; text layer only, no OCR; layout and reading order may differ");
    if !empty.is_empty() {
        out.push_str(&format!(
            "; no text extracted on page(s) {} (images, scans or unsupported fonts are not read)",
            page_list(&empty)
        ));
    }
    out.push(']');
    for (i, page) in pages.iter().enumerate().filter(|(_, p)| !p.is_empty()) {
        out.push_str(&format!("\n\n# Page {}\n", i + 1));
        for line in page {
            // A line that looks like a page heading would fake provenance.
            if line.starts_with('#') {
                out.push('\\');
            }
            out.push_str(line);
            out.push('\n');
        }
        if out.len() > MAX_OUTPUT_BYTES {
            return Err(PdfFailure::Limit);
        }
    }
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    Ok(out)
}

/// Builds small synthetic PDFs for tests: one Helvetica text page per entry,
/// each string drawn line by line. No external fixtures.
#[cfg(any(test, feature = "test-support"))]
pub fn fixture_pdf(pages: &[&[&str]], encrypt_marker: bool) -> Vec<u8> {
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Object, Stream};
    let mut doc = lopdf::Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let resources_id = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
    let mut kids = Vec::new();
    for lines in pages {
        let mut ops = vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 11.into()]),
            Operation::new("Td", vec![72.into(), 720.into()]),
        ];
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                ops.push(Operation::new("Td", vec![0.into(), (-14).into()]));
            }
            ops.push(Operation::new("Tj", vec![Object::string_literal(*line)]));
        }
        ops.push(Operation::new("ET", vec![]));
        let content = Content { operations: ops };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
            "Resources" => resources_id, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        kids.push(Object::from(page_id));
    }
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    if encrypt_marker {
        let enc = doc.add_object(dictionary! { "Filter" => "Standard", "V" => 1, "R" => 2 });
        doc.trailer.set("Encrypt", enc);
    }
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    #[test]
    fn pages_render_in_order_with_page_headings() {
        let pdf = fixture_pdf(&[&["Alpha line one", "alpha line two"], &["Beta NEEDLE_TOKEN"]], false);
        let text = extract_in_process(&pdf, soon()).unwrap();
        assert!(
            text.starts_with("[PDF text: 2 pages; text layer only, no OCR"),
            "{text}"
        );
        let one = text.find("# Page 1\nAlpha line one\nalpha line two").unwrap();
        let two = text.find("# Page 2\nBeta NEEDLE_TOKEN").unwrap();
        assert!(one < two);
    }

    #[test]
    fn empty_pages_are_disclosed_and_all_empty_is_refused() {
        let pdf = fixture_pdf(&[&["Only text"], &[], &["More"]], false);
        let text = extract_in_process(&pdf, soon()).unwrap();
        assert!(text.contains("no text extracted on page(s) 2 (images"), "{text}");
        assert!(!text.contains("# Page 2"));
        let blank = fixture_pdf(&[&[], &[]], false);
        assert_eq!(extract_in_process(&blank, soon()), Err(PdfFailure::NoText));
    }

    #[test]
    fn encrypted_malformed_and_oversized_inputs_are_refused() {
        assert_eq!(
            extract_in_process(&fixture_pdf(&[&["x"]], true), soon()),
            Err(PdfFailure::Encrypted)
        );
        assert_eq!(
            extract_in_process(b"%PDF-1.7 not really", soon()),
            Err(PdfFailure::Malformed)
        );
        assert_eq!(extract_in_process(b"hello", soon()), Err(PdfFailure::Malformed));
        let mut big = b"%PDF-1.4\n".to_vec();
        big.resize(MAX_INPUT_BYTES + 1, b' ');
        assert_eq!(extract_in_process(&big, soon()), Err(PdfFailure::Limit));
        let pages: Vec<&[&str]> = vec![&["p"]; MAX_PAGES + 1];
        assert_eq!(
            extract_in_process(&fixture_pdf(&pages, false), soon()),
            Err(PdfFailure::Limit)
        );
        let past = Instant::now() - Duration::from_millis(1);
        assert_eq!(
            extract_in_process(&fixture_pdf(&[&["x"]], false), past),
            Err(PdfFailure::Timeout)
        );
    }

    #[test]
    fn normalization_folds_ligatures_joins_hyphens_and_drops_furniture() {
        let lines = normalize_page("The \u{FB01}rst exam-\nple   has\u{00AD} gaps\n\n\n\nEnd -\nNext");
        assert_eq!(lines, vec!["The first example has gaps", "", "End -", "Next"]);
        let mut pages: Vec<Vec<String>> = (1..=5)
            .map(|n| vec!["Journal of Tests".into(), format!("Body {n}"), format!("Page {n} of 5")])
            .collect();
        strip_furniture(&mut pages);
        assert!(pages
            .iter()
            .enumerate()
            .all(|(i, p)| p == &vec![format!("Body {}", i + 1)]));
        assert!(undecodable("\u{FFFD}\u{FFFD}a"));
        assert!(!undecodable("plain text \u{FFFD}"));
    }

    #[test]
    fn distinct_numbers_at_page_edges_are_content_not_furniture() {
        let page = |first: String, n: usize| vec![first, format!("body {n}"), format!("{n}")];
        let mut pages: Vec<Vec<String>> = (1..=4).map(|n| page(format!("Revenue: ${n}00"), n)).collect();
        strip_furniture(&mut pages);
        // The bare page numbers go; every revenue figure stays.
        for (i, p) in pages.iter().enumerate() {
            assert_eq!(p, &vec![format!("Revenue: ${}00", i + 1), format!("body {}", i + 1)]);
        }
        let mut only: Vec<Vec<String>> = (1..=4).map(|n| vec![format!("Revenue: ${n}00")]).collect();
        strip_furniture(&mut only);
        assert!(only.iter().all(|p| p.len() == 1), "{only:?}");
        // A header carrying a constant volume and a running page number still goes.
        let words = ["alpha", "beta", "gamma", "delta"];
        let mut headed: Vec<Vec<String>> = (1..=4)
            .map(|n| vec![format!("Vol. 3, p. {}", n + 10), format!("text {}", words[n - 1])])
            .collect();
        strip_furniture(&mut headed);
        assert!(
            headed.iter().zip(words).all(|(p, w)| p == &vec![format!("text {w}")]),
            "{headed:?}"
        );
    }

    #[test]
    fn graphics_state_restore_returns_to_the_saved_font() {
        let mut fonts = BTreeMap::new();
        fonts.insert(
            b"F1".to_vec(),
            FontCodec::Mapped(lopdf::Encoding::SimpleEncoding(b"WinAnsiEncoding")),
        );
        let mut w = TextWalker::default();
        w.apply("Tf", &["F1".into(), 10.into()], &fonts);
        w.apply("q", &[], &fonts);
        w.apply("Tf", &["Missing".into(), 4.into()], &fonts);
        assert!(w.encoding.is_none());
        w.apply("Q", &[], &fonts);
        assert!(w.encoding.is_some());
        assert_eq!(w.font_size, 10.0);
        w.apply("Q", &[], &fonts);
        assert!(w.encoding.is_some(), "an unbalanced Q keeps the current font");
    }

    #[test]
    fn symbol_and_dingbats_fonts_use_their_built_in_encodings() {
        let font = |base: &str, extra: Option<(&str, lopdf::Object)>| {
            let mut d = lopdf::dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => base };
            if let Some((k, v)) = extra {
                d.set(k, v);
            }
            d
        };
        assert!(matches!(
            builtin_symbolic(&font("Symbol", None)),
            Some(FontCodec::Symbol)
        ));
        assert!(matches!(
            builtin_symbolic(&font("ABCDEF+SymbolMT", None)),
            Some(FontCodec::Symbol)
        ));
        assert!(matches!(
            builtin_symbolic(&font("ZapfDingbats", None)),
            Some(FontCodec::Dingbats)
        ));
        assert!(builtin_symbolic(&font("Helvetica", None)).is_none());
        // An explicit encoding or Unicode map is the font's own; keep lopdf's reading.
        let mapped = font("Symbol", Some(("Encoding", "WinAnsiEncoding".into())));
        assert!(builtin_symbolic(&mapped).is_none());

        let symbol = |bytes: &[u8]| bytes.iter().filter_map(|b| symbol_char(*b)).collect::<String>();
        assert_eq!(symbol(b"abg DSW p=3.14"), "αβγ ΔΣΩ π=3.14");
        assert_eq!(symbol(&[0xA3, 0xB1, 0xA5, 0xE5, 0xF2, 0xD6, 0xB9, 0xCE]), "≤±∞∑∫√≠∈");
        let dingbats = |bytes: &[u8]| bytes.iter().filter_map(|b| dingbat_char(*b)).collect::<String>();
        assert_eq!(
            dingbats(b"4 Yes 8 No n"),
            "✔ ••• ✘ •❏ ■",
            "never the digit 4 or a letter"
        );

        let mut fonts = BTreeMap::new();
        fonts.insert(b"S".to_vec(), FontCodec::Symbol);
        let mut w = TextWalker::default();
        w.apply("Tf", &["S".into(), 12.into()], &fonts);
        w.apply("Tj", &[lopdf::Object::string_literal("a+b")], &fonts);
        assert_eq!(w.out, "α+β");
    }

    #[test]
    fn heading_lookalikes_cannot_fake_page_provenance() {
        let pdf = fixture_pdf(&[&["# Page 9", "body"]], false);
        let text = extract_in_process(&pdf, soon()).unwrap();
        assert!(text.contains("# Page 1\n\\# Page 9\nbody"), "{text}");
    }

    #[test]
    fn walker_spaces_kerning_gaps_and_breaks_on_line_moves() {
        use lopdf::Object;
        let enc = BTreeMap::new();
        let mut w = TextWalker::default();
        w.apply(
            "Tm",
            &[1.into(), 0.into(), 0.into(), 1.into(), 72.into(), 700.into()],
            &enc,
        );
        w.out.push_str("Hello");
        w.apply("TJ", &[Object::Array(vec![(-400).into(), Object::Real(-20.0)])], &enc);
        w.out.push_str("world");
        w.apply(
            "Tm",
            &[1.into(), 0.into(), 0.into(), 1.into(), 72.into(), 686.into()],
            &enc,
        );
        w.out.push_str("next");
        w.apply(
            "Tm",
            &[1.into(), 0.into(), 0.into(), 1.into(), 140.into(), 686.into()],
            &enc,
        );
        w.out.push_str("same");
        assert_eq!(w.out, "Hello world\nnext same");
    }

    #[test]
    fn line_breaks_are_em_relative_and_justified_word_gaps_are_spaces() {
        use lopdf::Object;
        let enc = BTreeMap::new();
        let mut w = TextWalker::default();
        w.apply("Tf", &["F1".into(), 10.into()], &enc);
        w.out.push_str("Vaswani");
        // A 3-unit superscript raise at 10pt is under half an em: same line.
        w.apply("Td", &[30.into(), 3.into()], &enc);
        w.out.push('*');
        w.apply("Td", &[0.into(), (-14).into()], &enc);
        w.out.push_str("The");
        // pdfTeX justification: -180 is a word gap, -40 is kerning.
        w.apply("TJ", &[Object::Array(vec![(-180).into(), (-40).into()])], &enc);
        w.out.push_str("first");
        assert_eq!(w.out, "Vaswani *\nThe first");
        // Under a scaled text matrix the same rule holds in user space.
        let mut w = TextWalker::default();
        w.apply("Tf", &["F1".into(), 1.into()], &enc);
        w.apply(
            "Tm",
            &[9.into(), 0.into(), 0.into(), 9.into(), 72.into(), 700.into()],
            &enc,
        );
        w.out.push('a');
        w.apply("Td", &[0.into(), Object::Real(-1.2)], &enc);
        w.out.push('b');
        assert_eq!(w.out, "a\nb");
    }

    #[test]
    fn a_page_number_inside_a_running_header_is_furniture_too() {
        let words = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"];
        let mut pages: Vec<Vec<String>> = (1..=6)
            .map(|n| {
                let w = words[n - 1];
                vec![
                    "Journal of Results, Vol. 3".into(),
                    n.to_string(),
                    format!("{w} opens"),
                    format!("{w} ends"),
                ]
            })
            .collect();
        strip_furniture(&mut pages);
        assert!(
            pages
                .iter()
                .zip(words)
                .all(|(p, w)| p == &vec![format!("{w} opens"), format!("{w} ends")]),
            "{pages:?}"
        );
        for bare in ["3", "- 3 -", "Page 3", "p. 3", "3 of 40", "3 | 40"] {
            assert!(bare_page_number(bare), "{bare}");
        }
        for prose in ["Chapter 3", "Table 3 shows", "3.5 GHz", ""] {
            assert!(!bare_page_number(prose), "{prose}");
        }
        // Page 9 and page 10 are the same furniture.
        let mut long: Vec<Vec<String>> = (1..=15)
            .map(|n| vec![format!("{} of the body", words[n % 6]), n.to_string()])
            .collect();
        strip_furniture(&mut long);
        assert!(long.iter().all(|p| p.len() == 1), "{long:?}");
    }

    #[test]
    fn table_cells_on_one_baseline_stay_on_one_line() {
        let mut fonts = BTreeMap::new();
        fonts.insert(
            b"F1".to_vec(),
            FontCodec::Mapped(lopdf::Encoding::SimpleEncoding(b"WinAnsiEncoding")),
        );
        let mut w = TextWalker::default();
        let at =
            |x: i64, y: i64| -> Vec<lopdf::Object> { vec![1.into(), 0.into(), 0.into(), 1.into(), x.into(), y.into()] };
        let tj = |t: &str| vec![lopdf::Object::string_literal(t)];
        w.apply("Tf", &["F1".into(), 10.into()], &fonts);
        // How report generators draw a table: one BT/Tm/Tj/T*/ET per cell.
        for (y, row) in [(700, ["Item", "Qty", "$3.50"]), (686, ["Widget", "2", "$7.00"])] {
            for (i, cell) in row.iter().enumerate() {
                w.apply("Tm", &at(72 + 80 * i as i64, y), &fonts);
                w.apply("Tj", &tj(cell), &fonts);
                w.apply("T*", &[], &fonts);
            }
        }
        // A paragraph's T* lines still break.
        w.apply("Tm", &at(72, 600), &fonts);
        w.apply("Tj", &tj("first line"), &fonts);
        w.apply("T*", &[], &fonts);
        w.apply("Tj", &tj("second line"), &fonts);
        assert_eq!(w.out, "Item Qty $3.50\nWidget 2 $7.00\nfirst line\nsecond line");
    }

    #[test]
    fn extract_caches_by_digest_and_forget_drops_it() {
        let pdf = fixture_pdf(&[&["cache me"]], false);
        let first = extract(&pdf, soon()).unwrap();
        let again = extract(&pdf, soon()).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        forget(&[crate::common::sha256_bytes_hex(&pdf)]);
        let fresh = extract(&pdf, soon()).unwrap();
        assert!(!Arc::ptr_eq(&first, &fresh));
        assert_eq!(first, fresh);
        assert_eq!(extract(b"not a pdf", soon()).unwrap_err().code, "PDF_MALFORMED");
    }

    #[test]
    fn refusal_messages_are_fixed_and_unknown_codes_are_generic() {
        assert_eq!(refusal("PDF_ENCRYPTED").message, "encrypted PDFs are not supported");
        assert_eq!(refusal("lopdf said: /secret.pdf").code, "PDF_WORKER_FAILED");
    }
}
