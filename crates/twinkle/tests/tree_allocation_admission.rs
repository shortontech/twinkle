//! Isolated allocator cross-check for retained-tree accounting.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};

use twinkle_ui::{Column, Container, FrameRequest, Rect, Text, UiFrame, UiStateStore};

struct CountingAllocator;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicIsize = AtomicIsize::new(0);
static PEAK_BYTES: AtomicIsize = AtomicIsize::new(0);

fn add_live(bytes: isize) {
    let live = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    let mut peak = PEAK_BYTES.load(Ordering::Relaxed);
    while live > peak {
        match PEAK_BYTES.compare_exchange_weak(peak, live, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => peak = actual,
        }
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && ACTIVE.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            add_live(layout.size() as isize);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if ACTIVE.load(Ordering::Relaxed) {
            add_live(-(layout.size() as isize));
        }
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        if !replacement.is_null() && ACTIVE.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            add_live(new_size as isize - layout.size() as isize);
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn native_cell_tree(cells: usize) -> Column<()> {
    let mut root = Column::new().width(640.0).height(480.0);
    for index in 0..cells {
        root = root.child(
            Container::new()
                .id(format!("cell-{index}"))
                .width(24.0)
                .height(24.0)
                .child(Text::new(if index % 17 == 0 { "active" } else { "" })),
        );
    }
    root
}

fn reset_counter() {
    ACTIVE.store(false, Ordering::SeqCst);
    ALLOCATIONS.store(0, Ordering::Relaxed);
    LIVE_BYTES.store(0, Ordering::Relaxed);
    PEAK_BYTES.store(0, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::SeqCst);
}

#[test]
#[ignore = "release-profile allocator cross-check"]
fn retained_tree_estimate_is_cross_checked_against_live_allocator_bytes() {
    // Warm global font/layout caches before isolating the tree lifetime.
    let mut warm_state = UiStateStore::default();
    drop(UiFrame::resolve(
        native_cell_tree(800),
        FrameRequest::new(Rect::new(0.0, 0.0, 640.0, 480.0), &mut warm_state),
    ));

    for cells in [50, 200, 800] {
        let mut state = UiStateStore::default();
        reset_counter();
        let frame = Box::new(UiFrame::resolve(
            native_cell_tree(cells),
            FrameRequest::new(Rect::new(0.0, 0.0, 640.0, 480.0), &mut state),
        ));
        let resources = frame.resource_diagnostics();
        let live_bytes = LIVE_BYTES.load(Ordering::Relaxed).max(0) as usize;
        let peak_bytes = PEAK_BYTES.load(Ordering::Relaxed).max(0) as usize;
        let allocations = ALLOCATIONS.load(Ordering::Relaxed);
        ACTIVE.store(false, Ordering::SeqCst);

        println!(
            "tree_allocator_cross_check nodes={} estimated_owned_bytes={} allocator_live_bytes={} allocator_peak_bytes={} allocator_calls={} residual_bytes={}",
            resources.node_count,
            resources.estimated_owned_bytes,
            live_bytes,
            peak_bytes,
            allocations,
            live_bytes as isize - resources.estimated_owned_bytes as isize,
        );
        assert_eq!(resources.node_count, cells * 2 + 1);
        assert!(resources.retained_id_index_bytes > 0);
        assert!(resources.hash_table_overhead_bytes > 0);
        assert!(resources.estimated_owned_bytes > 0);
        assert!(live_bytes > 0);
        assert!(peak_bytes >= live_bytes);
        assert!(allocations > 0);
        assert!(resources.estimated_owned_bytes <= live_bytes);
        assert!(
            live_bytes - resources.estimated_owned_bytes <= live_bytes / 10,
            "categorized estimate diverged by more than 10% from allocator-visible live bytes"
        );
    }
}
