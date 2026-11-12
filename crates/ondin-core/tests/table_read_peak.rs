//! A JSON table's read holds one parse of the file at a time. Its own test
//! binary, since it counts every allocation the process makes.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use ondin_core::chart::table_io::{Format, read};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to `System` with the same layout; the counters
// only observe.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(live, Ordering::Relaxed);
        // SAFETY: the caller's layout is passed on unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: `ptr` came from `alloc` above with this layout.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// Reading many one-key objects peaks well under two `serde_json` trees of the
/// file: about 12 bytes per byte of `{"a":1},` read once, near 100 with a
/// `serde_json::Value` tree held beside it.
/// flip: `read_json` parsing into `serde_json::Value` first and keeping it → fails at the peak bound.
#[test]
fn a_json_read_holds_one_parse_of_the_file() {
    let n = 100_000;
    let mut text = String::with_capacity(n * 8 + 2);
    text.push('[');
    for i in 0..n {
        if i > 0 {
            text.push(',');
        }
        text.push_str(r#"{"a":1}"#);
    }
    text.push(']');
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let r = read(&text, Format::Json, None).unwrap();
    let peak = PEAK.load(Ordering::Relaxed) - before;
    assert_eq!(r.table.columns.len(), 1);
    assert!(
        peak < 30 * text.len(),
        "peak {peak} bytes for a {}-byte file",
        text.len()
    );
}
