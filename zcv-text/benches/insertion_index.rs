use criterion::{BatchSize, BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, TransactionMetadata};

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

fn insertion_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("insertion_index");

    for count in [64, 256, 1024] {
        let buffer = buffer_with_insertions(count);
        group.bench_with_input(BenchmarkId::new("snapshot", count), &buffer, |b, buffer| {
            b.iter(|| black_box(buffer.snapshot()));
        });
        group.bench_with_input(BenchmarkId::new("edit", count), &count, |b, count| {
            b.iter_batched(
                || buffer_with_insertions(*count),
                |mut buffer| {
                    buffer
                        .edit(
                            [Edit::insert(ByteOffset::new(*count), "x").unwrap()],
                            TransactionMetadata::default(),
                        )
                        .unwrap();
                    black_box(buffer.version());
                },
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, insertion_index);
criterion_main!(benches);
