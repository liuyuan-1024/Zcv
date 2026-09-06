use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use zcv_benchmarks::cached_rust_document;
use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, Line, SearchQuery, TransactionMetadata};

const DOCUMENT_SIZES: [usize; 3] = [64 * 1024, 1024 * 1024, 16 * 1024 * 1024];

fn buffers(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_buffer/create");

    for size in DOCUMENT_SIZES {
        let text = cached_rust_document(size);
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(text.len()), &text, |b, text| {
            b.iter(|| {
                Buffer::from_text(black_box(text.to_string()), BufferConfig::default()).unwrap()
            });
        });
    }

    group.finish();
}

fn editing(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_buffer/edit_at_middle");

    for size in DOCUMENT_SIZES {
        let text = cached_rust_document(size);
        let offset = ByteOffset::new(text.len() / 2);
        group.throughput(Throughput::Bytes("let inserted = true;\n".len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(text.len()), &text, |b, text| {
            b.iter_batched(
                || Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap(),
                |mut buffer| {
                    buffer
                        .edit(
                            [Edit::insert(offset, "let inserted = true;\n").unwrap()],
                            TransactionMetadata::default(),
                        )
                        .unwrap();
                    black_box(buffer.version());
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

fn searches(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_buffer/search");

    for size in DOCUMENT_SIZES {
        let text = cached_rust_document(size);
        let byte_len = text.len();
        let snapshot = Buffer::from_text(text.to_string(), BufferConfig::default())
            .unwrap()
            .snapshot();
        group.bench_with_input(
            BenchmarkId::new("literal", byte_len),
            &snapshot,
            |b, snapshot| {
                b.iter(|| black_box(snapshot.search_literal("render_document").unwrap().len()));
            },
        );

        let query = SearchQuery {
            query: "render_document\\(index: usize\\)".to_string(),
            regex: true,
            ..Default::default()
        }
        .prepare()
        .unwrap();
        group.bench_with_input(
            BenchmarkId::new("prepared_regex", byte_len),
            &snapshot,
            |b, snapshot| {
                b.iter(|| black_box(query.search(snapshot).unwrap().matches().len()));
            },
        );
    }

    group.finish();
}

fn coordinates(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_buffer/coordinate_conversion");

    for size in DOCUMENT_SIZES {
        let text = cached_rust_document(size);
        let byte_len = text.len();
        let snapshot = Buffer::from_text(text.to_string(), BufferConfig::default())
            .unwrap()
            .snapshot();
        let positions = [0, snapshot.line_count() / 2, snapshot.line_count() - 2].map(|line| {
            snapshot
                .byte_to_position(snapshot.line_start_byte(Line::new(line)).unwrap())
                .unwrap()
        });
        group.bench_with_input(
            BenchmarkId::new("position_to_byte", byte_len),
            &(snapshot, positions),
            |b, (snapshot, positions)| {
                b.iter(|| {
                    for position in positions {
                        black_box(snapshot.position_to_byte(*position).unwrap());
                    }
                });
            },
        );
    }

    group.finish();
}

criterion_group!(text_buffer_benches, buffers, editing, searches, coordinates);
criterion_main!(text_buffer_benches);
