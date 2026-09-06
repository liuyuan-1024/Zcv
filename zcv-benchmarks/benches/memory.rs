use std::alloc::{GlobalAlloc, Layout, System};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde::Serialize;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System as ProcessSystem, get_current_pid};
use zcv_benchmarks::rust_document;
use zcv_language::highlight_snippet;
use zcv_text::{Buffer, BufferConfig};

struct CountingAllocator;

static MEASURING: AtomicBool = AtomicBool::new(false);
static ALLOCATION_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        record_deallocation(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(pointer, layout, new_size) };
        if !next.is_null() && MEASURING.load(Ordering::Relaxed) {
            if new_size >= layout.size() {
                record_allocation(new_size - layout.size());
            } else {
                record_deallocation(layout.size() - new_size);
            }
        }
        next
    }
}

fn record_allocation(size: usize) {
    if !MEASURING.load(Ordering::Relaxed) {
        return;
    }
    ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
    ALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
    let live = LIVE_BYTES.fetch_add(size, Ordering::Relaxed) + size;
    PEAK_LIVE_BYTES.fetch_max(live, Ordering::Relaxed);
}

fn record_deallocation(size: usize) {
    if !MEASURING.load(Ordering::Relaxed) {
        return;
    }
    let _ = LIVE_BYTES.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        Some(live.saturating_sub(size))
    });
}

#[derive(Serialize)]
struct MemorySample {
    name: String,
    input_bytes: usize,
    rss_before_bytes: u64,
    rss_with_result_bytes: u64,
    rss_after_drop_bytes: u64,
    allocation_count: usize,
    allocated_bytes: usize,
    peak_live_allocation_bytes: usize,
}

struct RssMeter {
    process_system: ProcessSystem,
    pid: sysinfo::Pid,
}

impl RssMeter {
    fn new() -> Self {
        Self {
            process_system: ProcessSystem::new(),
            pid: get_current_pid().expect("应获取当前进程 PID"),
        }
    }

    fn current(&mut self) -> u64 {
        self.process_system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        self.process_system
            .process(self.pid)
            .map_or(0, sysinfo::Process::memory)
    }
}

fn measure<T>(
    rss: &mut RssMeter,
    name: impl Into<String>,
    input_bytes: usize,
    operation: impl FnOnce() -> T,
) -> MemorySample {
    let rss_before_bytes = rss.current();
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    LIVE_BYTES.store(0, Ordering::Relaxed);
    PEAK_LIVE_BYTES.store(0, Ordering::Relaxed);
    MEASURING.store(true, Ordering::SeqCst);
    let result = operation();
    MEASURING.store(false, Ordering::SeqCst);
    let rss_with_result_bytes = rss.current();
    drop(result);

    MemorySample {
        name: name.into(),
        input_bytes,
        rss_before_bytes,
        rss_with_result_bytes,
        rss_after_drop_bytes: rss.current(),
        allocation_count: ALLOCATION_COUNT.load(Ordering::Relaxed),
        allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
        peak_live_allocation_bytes: PEAK_LIVE_BYTES.load(Ordering::Relaxed),
    }
}

fn main() {
    let mut rss = RssMeter::new();
    let mut samples = Vec::new();

    for size in [64 * 1024, 1024 * 1024, 16 * 1024 * 1024] {
        let text = rust_document(size);
        let input_bytes = text.len();
        samples.push(measure(
            &mut rss,
            format!("text_buffer/create/{input_bytes}"),
            input_bytes,
            || Buffer::from_text(text.clone(), BufferConfig::default()).expect("应创建 Buffer"),
        ));

        let snapshot = Buffer::from_text(text.clone(), BufferConfig::default())
            .expect("应创建搜索快照")
            .snapshot();
        samples.push(measure(
            &mut rss,
            format!("text_buffer/search_literal/{input_bytes}"),
            input_bytes,
            || {
                snapshot
                    .search_literal("render_document")
                    .expect("搜索应成功")
            },
        ));

        samples.push(measure(
            &mut rss,
            format!("language/highlight_rust_document/{input_bytes}"),
            input_bytes,
            || highlight_snippet("rust", &text).expect("Rust 高亮应成功"),
        ));
    }

    let output_dir = std::env::var_os("ZCV_MEMORY_BENCHMARK_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".performance/memory"));
    fs::create_dir_all(&output_dir).expect("应创建内存基准结果目录");
    let label =
        std::env::var("ZCV_MEMORY_BENCHMARK_LABEL").unwrap_or_else(|_| "current".to_owned());
    assert!(
        label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "内存基准记录名只能包含 ASCII 字母、数字、连字符和下划线"
    );
    let output_path = output_dir.join(format!("{label}.json"));
    fs::write(
        &output_path,
        serde_json::to_vec_pretty(&samples).expect("应序列化内存基准结果"),
    )
    .expect("应写入内存基准结果");

    for sample in &samples {
        println!(
            "{}: RSS {} -> {} -> {} MiB, 分配 {} 次 / {} MiB, 峰值活跃分配 {} MiB",
            sample.name,
            sample.rss_before_bytes / 1024 / 1024,
            sample.rss_with_result_bytes / 1024 / 1024,
            sample.rss_after_drop_bytes / 1024 / 1024,
            sample.allocation_count,
            sample.allocated_bytes / 1024 / 1024,
            sample.peak_live_allocation_bytes / 1024 / 1024,
        );
    }
    println!("内存基准结果已写入 {}", output_path.display());
}
