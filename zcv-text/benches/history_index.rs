use criterion::{BatchSize, BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use zcv_text::{ByteOffset, Edit, TransactionMetadata};

#[path = "common/history.rs"]
mod history;

use history::buffer_with_history_steps;

fn history_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("history_index/append");
    for count in [64, 256, 1024] {
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, count| {
            b.iter_batched(
                || buffer_with_history_steps(*count),
                |mut buffer| {
                    buffer
                        .edit(
                            [Edit::insert(ByteOffset::new(*count), "x").unwrap()],
                            TransactionMetadata::default().without_history(),
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

criterion_group!(benches, history_index);
criterion_main!(benches);
