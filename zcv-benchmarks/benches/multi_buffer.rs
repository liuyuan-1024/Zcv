use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use gpui::{AppContext as _, TestAppContext, TestDispatcher};
use zcv_benchmarks::cached_rust_document;
use zcv_language::LanguageBuffer;
use zcv_multi_buffer::{MultiBuffer, MultiBufferExcerpt};
use zcv_text::{Buffer, BufferConfig};

const SOURCE_COUNTS: [usize; 2] = [2, 16];
const SOURCE_BYTES: usize = 1024 * 1024;

fn projection_setup(
    source_count: usize,
) -> (
    TestAppContext,
    gpui::Entity<MultiBuffer>,
    Vec<MultiBufferExcerpt>,
) {
    let mut cx = TestAppContext::build(TestDispatcher::new(1), None);
    let sources = (0..source_count)
        .map(|_| {
            let buffer = cx.new(|_| {
                Buffer::scratch(
                    cached_rust_document(SOURCE_BYTES).to_string(),
                    BufferConfig::default(),
                )
                .unwrap()
            });
            cx.new(|cx| LanguageBuffer::new(buffer, None, cx))
        })
        .collect::<Vec<_>>();
    let excerpts = cx.read(|cx| {
        sources
            .iter()
            .map(|source| {
                let line_count = source.read(cx).text_snapshot(cx).line_count();
                MultiBufferExcerpt::line_range(source.clone(), 0..line_count, cx)
            })
            .collect()
    });
    let multi_buffer = cx.new(MultiBuffer::empty);
    (cx, multi_buffer, excerpts)
}

fn materialize_excerpts(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_buffer/materialize_excerpts");

    for source_count in SOURCE_COUNTS {
        group.throughput(Throughput::Bytes((source_count * SOURCE_BYTES) as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(source_count),
            &source_count,
            |b, &source_count| {
                b.iter_batched(
                    || projection_setup(source_count),
                    |(mut cx, multi_buffer, excerpts)| {
                        cx.update_entity(&multi_buffer, |multi_buffer, cx| {
                            multi_buffer.set_excerpts(excerpts, cx);
                        });
                        let snapshot = cx.read_entity(&multi_buffer, |multi_buffer, cx| {
                            multi_buffer.snapshot(cx)
                        });
                        black_box(snapshot.text().len_bytes());
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

criterion_group!(multi_buffer_benches, materialize_excerpts);
criterion_main!(multi_buffer_benches);
