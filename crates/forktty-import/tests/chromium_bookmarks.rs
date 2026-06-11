use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use forktty_import::read_chromium_bookmarks;

struct CountingAllocator;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATED.fetch_add(new_size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn chromium_bookmarks_skip_allocating_ignored_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Bookmarks");
    let metadata = "x".repeat(512 * 1024);
    let json = format!(
        r#"{{
            "roots": {{
                "bookmark_bar": {{
                    "children": [{{
                        "type": "url",
                        "name": "ForkTTY",
                        "url": "https://forktty.test/",
                        "meta_info": {{"huge": "{metadata}"}}
                    }}]
                }}
            }}
        }}"#
    );
    std::fs::write(&path, &json).unwrap();

    ALLOCATED.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    let bookmarks = read_chromium_bookmarks(&path).unwrap();
    COUNTING.store(false, Ordering::Relaxed);
    let allocated = ALLOCATED.load(Ordering::Relaxed);

    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].url, "https://forktty.test/");
    assert!(
        allocated < json.len() + 256 * 1024,
        "ignored metadata should be skipped during parsing; allocated {allocated} bytes for {} bytes of JSON",
        json.len()
    );
}
