use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use zcv_benchmarks::rust_document;
use zcv_language::highlight_snippet;

const DOCUMENT_SIZES: [usize; 3] = [64 * 1024, 1024 * 1024, 16 * 1024 * 1024];

fn highlight_rust_document(c: &mut Criterion) {
    let mut group = c.benchmark_group("language/highlight_rust_document");

    for size in DOCUMENT_SIZES {
        let text = rust_document(size);
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(text.len()), &text, |b, text| {
            b.iter(|| {
                let highlights = highlight_snippet("rust", black_box(text)).unwrap();
                black_box(highlights.spans.len());
            });
        });
    }

    group.finish();
}

criterion_group!(language_benches, highlight_rust_document);
criterion_main!(language_benches);
