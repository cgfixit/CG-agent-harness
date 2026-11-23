//! A heap ceiling that works on every OS. The `cgagentharness` binary installs
//! [`CappedAlloc`] as its global allocator; it is a pass-through to `System`
//! until a worker process calls [`arm`] once at start. Armed, it counts live
//! heap bytes and refuses an allocation that would pass the ceiling, which
//! aborts that child. Armed, every failed allocation (this ceiling, or the
//! system's under `RLIMIT_AS`) first writes [`REFUSAL_MARKER`] to stderr once:
//! the PDF parent reports `PDF_LIMIT` only for an abort that carries it.
//!
//! Unarmed cost: one relaxed atomic load per allocation and free. Bytes
//! allocated before arming and freed after it are not counted, so the bound
//! is "heap growth since arming", which in a worker is nearly all of it.
//! `RLIMIT_AS` still applies on Linux; this covers macOS and Windows, where
//! the kill deadline was the only bound.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// 0 = unarmed. Set once, never lowered back to 0.
static LIMIT: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// Set by the first failed allocation while armed.
static REFUSED: AtomicBool = AtomicBool::new(false);

/// The line an armed process writes to stderr when an allocation first fails.
/// It is the in-band proof that the abort that usually follows was the heap
/// bound, not a bug, a double panic or a stack overflow, which abort the same way.
pub const REFUSAL_MARKER: &str = "cgagentharness: armed allocation refused";

/// Notes a failed allocation. Armed only, once per process, and without
/// allocating: a raw `write(2)` of a static line (Unix; Windows reports no
/// signal, so its parent never reads an abort as a limit and needs no line).
fn note_refusal() {
    let first = LIMIT.load(Ordering::Relaxed) != 0 && !REFUSED.swap(true, Ordering::Relaxed);
    // SAFETY: fd 2 and a static buffer of `MARKER_LINE.len()` bytes; a short or
    // failed write only loses the marker, and the parent then fails closed.
    #[cfg(unix)]
    if first {
        let _ = unsafe { libc::write(2, MARKER_LINE.as_ptr().cast(), MARKER_LINE.len()) };
    }
    #[cfg(not(unix))]
    let _ = first;
}

/// [`REFUSAL_MARKER`] on a line of its own, whatever stderr held before it.
#[cfg(unix)]
const MARKER_LINE: &[u8] = b"\ncgagentharness: armed allocation refused\n";

pub struct CappedAlloc;

/// Starts counting with `limit` bytes of heap. Only a worker child calls this.
/// A second call keeps the first ceiling.
pub fn arm(limit: usize) {
    let _ = LIMIT.compare_exchange(0, limit.max(1), Ordering::SeqCst, Ordering::SeqCst);
}

/// Live bytes counted since [`arm`]; 0 while unarmed.
#[cfg(test)]
fn live_bytes() -> usize {
    LIVE.load(Ordering::Relaxed)
}

fn reserve(size: usize) -> bool {
    let limit = LIMIT.load(Ordering::Relaxed);
    if limit == 0 {
        return true;
    }
    LIVE.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        live.checked_add(size).filter(|next| *next <= limit)
    })
    .is_ok()
}

fn release(size: usize) {
    if LIMIT.load(Ordering::Relaxed) == 0 {
        return;
    }
    // Saturating: a block allocated before arming may be freed after it.
    let _ = LIVE.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        Some(live.saturating_sub(size))
    });
}

// SAFETY: every method forwards to `System` with the caller's layout unchanged;
// the counter only decides whether to forward, and a refusal returns null,
// which the `GlobalAlloc` contract permits.
unsafe impl GlobalAlloc for CappedAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            note_refusal();
            return std::ptr::null_mut();
        }
        // SAFETY: same layout the caller passed, which it guarantees is valid.
        let ptr = unsafe { System.alloc(layout) };
        if ptr.is_null() {
            release(layout.size());
            note_refusal();
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            note_refusal();
            return std::ptr::null_mut();
        }
        // SAFETY: as in `alloc`.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if ptr.is_null() {
            release(layout.size());
            note_refusal();
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller passes a block this allocator (hence `System`) returned with `layout`.
        unsafe { System.dealloc(ptr, layout) };
        release(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old = layout.size();
        if new_size > old && !reserve(new_size - old) {
            note_refusal();
            return std::ptr::null_mut();
        }
        // SAFETY: the caller upholds `realloc`'s contract for `ptr`, `layout` and `new_size`.
        let next = unsafe { System.realloc(ptr, layout, new_size) };
        if next.is_null() {
            if new_size > old {
                release(new_size - old);
            }
            note_refusal();
        } else if new_size < old {
            release(old - new_size);
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The counter is process-global and unit tests share one process, so the
    // arming test drives the allocator directly instead of installing it.
    #[test]
    fn armed_allocator_refuses_past_the_ceiling_and_counts_frees() {
        let a = CappedAlloc;
        let small = Layout::from_size_align(1024, 8).unwrap();
        // Unarmed: pass-through, nothing counted.
        let p = unsafe { a.alloc(small) };
        assert!(!p.is_null());
        assert_eq!(live_bytes(), 0);
        arm(4096);
        arm(1 << 40); // a second arm keeps the first ceiling
        let q = unsafe { a.alloc(small) };
        assert!(!q.is_null());
        assert_eq!(live_bytes(), 1024);
        assert!(!REFUSED.load(Ordering::Relaxed), "nothing refused yet");
        let big = Layout::from_size_align(8192, 8).unwrap();
        assert!(unsafe { a.alloc(big) }.is_null(), "past the ceiling");
        assert!(REFUSED.load(Ordering::Relaxed), "a refusal while armed is noted");
        assert_eq!(live_bytes(), 1024, "a refusal reserves nothing");
        let grown = unsafe { a.realloc(q, small, 3000) };
        assert!(!grown.is_null());
        assert_eq!(live_bytes(), 3000);
        assert!(unsafe { a.realloc(grown, Layout::from_size_align(3000, 8).unwrap(), 5000) }.is_null());
        assert_eq!(live_bytes(), 3000);
        unsafe { a.dealloc(grown, Layout::from_size_align(3000, 8).unwrap()) };
        assert_eq!(live_bytes(), 0);
        // A pre-arm block freed now saturates at zero instead of wrapping.
        unsafe { a.dealloc(p, small) };
        assert_eq!(live_bytes(), 0);
    }

    /// The parent matches the marker by text; the bytes written are that text.
    #[cfg(unix)]
    #[test]
    fn the_written_line_is_the_marker_on_a_line_of_its_own() {
        let line = std::str::from_utf8(MARKER_LINE).unwrap();
        assert_eq!(line, format!("\n{REFUSAL_MARKER}\n"));
    }
}
