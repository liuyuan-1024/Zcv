use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, TransactionMetadata};

struct CountingAllocator;

static MEASURING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && MEASURING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            let live = LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK_LIVE_BYTES.fetch_max(live, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        if MEASURING.load(Ordering::Relaxed) {
            let _ = LIVE_BYTES.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                Some(live.saturating_sub(layout.size()))
            });
        }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(pointer, layout, new_size) };
        if !next.is_null() && MEASURING.load(Ordering::Relaxed) {
            if new_size >= layout.size() {
                let increase = new_size - layout.size();
                ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
                ALLOCATED_BYTES.fetch_add(increase, Ordering::Relaxed);
                let live = LIVE_BYTES.fetch_add(increase, Ordering::Relaxed) + increase;
                PEAK_LIVE_BYTES.fetch_max(live, Ordering::Relaxed);
            } else {
                let _ = LIVE_BYTES.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                    Some(live.saturating_sub(layout.size() - new_size))
                });
            }
        }
        next
    }
}

fn buffer_with_insertions(count: usize) -> Buffer {
    let mut buffer = Buffer::from_text(String::new(), BufferConfig::default()).unwrap();
    for offset in 0..count {
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(offset), "x").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
    }
    buffer
}

fn main() {
    for count in [64, 256, 1024] {
        let mut buffer = buffer_with_insertions(count);
        ALLOCATIONS.store(0, Ordering::Relaxed);
        ALLOCATED_BYTES.store(0, Ordering::Relaxed);
        LIVE_BYTES.store(0, Ordering::Relaxed);
        PEAK_LIVE_BYTES.store(0, Ordering::Relaxed);

        MEASURING.store(true, Ordering::SeqCst);
        let snapshot = std::hint::black_box(buffer.snapshot());
        let allocations = ALLOCATIONS.load(Ordering::Relaxed);
        let allocated_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let peak_live_bytes = PEAK_LIVE_BYTES.load(Ordering::Relaxed);
        drop(snapshot);
        let live_after_drop = LIVE_BYTES.load(Ordering::Relaxed);
        MEASURING.store(false, Ordering::SeqCst);

        println!(
            "插入次数 {count}：快照分配 {allocations} 次，累计 {allocated_bytes} 字节，峰值新增 {peak_live_bytes} 字节，释放后新增 {live_after_drop} 字节"
        );

        ALLOCATIONS.store(0, Ordering::Relaxed);
        ALLOCATED_BYTES.store(0, Ordering::Relaxed);
        MEASURING.store(true, Ordering::SeqCst);
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(count), "x").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
        let edit_allocations = ALLOCATIONS.load(Ordering::Relaxed);
        let edit_allocated_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
        MEASURING.store(false, Ordering::SeqCst);
        println!(
            "插入次数 {count}：后续编辑分配 {edit_allocations} 次，累计 {edit_allocated_bytes} 字节"
        );
    }
}
