//! Process-wide Rust allocation requests only; foreign C++/driver allocations excluded.
#[cfg(feature = "pipeline-allocator")]
mod enabled {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        sync::atomic::{AtomicUsize, Ordering},
    };
    struct Counting;
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static FREES: AtomicUsize = AtomicUsize::new(0);
    static REQUESTED: AtomicUsize = AtomicUsize::new(0);
    fn add(size: usize) {
        let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
        PEAK.fetch_max(live, Ordering::Relaxed);
        CALLS.fetch_add(1, Ordering::Relaxed);
        REQUESTED.fetch_add(size, Ordering::Relaxed);
    }
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            let p = unsafe { System.alloc(l) };
            if !p.is_null() {
                add(l.size());
            }
            p
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            let p = unsafe { System.alloc_zeroed(l) };
            if !p.is_null() {
                add(l.size());
            }
            p
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
            FREES.fetch_add(1, Ordering::Relaxed);
            unsafe { System.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
            let n = unsafe { System.realloc(p, l, size) };
            if !n.is_null() {
                if size >= l.size() {
                    let live = LIVE.fetch_add(size - l.size(), Ordering::Relaxed) + size - l.size();
                    PEAK.fetch_max(live, Ordering::Relaxed);
                } else {
                    LIVE.fetch_sub(l.size() - size, Ordering::Relaxed);
                }
                CALLS.fetch_add(1, Ordering::Relaxed);
                REQUESTED.fetch_add(size, Ordering::Relaxed);
            }
            n
        }
    }
    #[global_allocator]
    static ALLOCATOR: Counting = Counting;
    pub fn counts() -> [u64; 3] {
        use Ordering::Relaxed;
        [
            CALLS.load(Relaxed) as u64,
            FREES.load(Relaxed) as u64,
            REQUESTED.load(Relaxed) as u64,
        ]
    }
    pub fn snapshot() -> serde_json::Value {
        use Ordering::Relaxed;
        serde_json::json!({"liveRequestedBytes":LIVE.load(Relaxed),"peakRequestedBytes":PEAK.load(Relaxed),"allocationOrReallocationCalls":CALLS.load(Relaxed),"deallocationCalls":FREES.load(Relaxed),"cumulativeRequestedBytes":REQUESTED.load(Relaxed),"scope":"Process-wide Rust System allocator requests; concurrent threads overlap. Excludes foreign V8/C++/driver allocations, allocator metadata, reserved arenas and fragmentation. Overlaps expanded frame ownership and RSS/PSS; do not sum."})
    }
}
pub(crate) fn snapshot() -> serde_json::Value {
    #[cfg(feature = "pipeline-allocator")]
    return enabled::snapshot();
    #[cfg(not(feature = "pipeline-allocator"))]
    serde_json::Value::Null
}

pub(crate) fn counts() -> Option<[u64; 3]> {
    #[cfg(feature = "pipeline-allocator")]
    return Some(enabled::counts());
    #[cfg(not(feature = "pipeline-allocator"))]
    None
}
