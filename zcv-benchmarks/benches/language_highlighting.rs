use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use zcv_benchmarks::{cached_injection_stress_document, cached_rust_document};
use zcv_language::highlight_snippet;

const DOCUMENT_SIZES: [usize; 3] = [64 * 1024, 1024 * 1024, 16 * 1024 * 1024];

/// 代表性 Rust 文档（宏密度贴近真实源码）的整块高亮，是解读高亮成本的默认档。
fn highlight_rust_document(c: &mut Criterion) {
    let mut group = c.benchmark_group("language/highlight_rust_document");

    for size in DOCUMENT_SIZES {
        let text = cached_rust_document(size);
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(text.len()), &text, |b, text| {
            b.iter(|| {
                let highlights = highlight_snippet("rust", black_box(text.as_ref())).unwrap();
                black_box(highlights.spans.len());
            });
        });
    }

    group.finish();
}

/// 注入压力测试：病态宏密集语料（约 88 字节/宏）刻意放大「宏 → 注入 rust 子解析」级联。
/// 用于压测注入引擎，非代表性负载；绝对数字须与 `highlight_rust_document` 对照解读。
fn highlight_injection_stress(c: &mut Criterion) {
    let mut group = c.benchmark_group("language/highlight_injection_stress");

    for size in DOCUMENT_SIZES {
        let text = cached_injection_stress_document(size);
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(text.len()), &text, |b, text| {
            b.iter(|| {
                let highlights = highlight_snippet("rust", black_box(text.as_ref())).unwrap();
                black_box(highlights.spans.len());
            });
        });
    }

    group.finish();
}

criterion_group!(
    language_benches,
    highlight_rust_document,
    highlight_injection_stress
);
criterion_main!(language_benches);
