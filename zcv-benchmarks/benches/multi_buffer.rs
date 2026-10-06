use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use std::path::PathBuf;
use std::time::Duration;

use gpui::{AppContext as _, TestAppContext, TestDispatcher};
mod common;

use common::cached_rust_document;
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{ExcerptRange, MultiBuffer, MultiBufferSnapshot};
use zcv_text::{Buffer, BufferConfig, ByteOffset, Line};

const SOURCE_COUNTS: [usize; 2] = [2, 16];
const SOURCE_BYTES: usize = 1024 * 1024;

fn projection_setup(
    source_count: usize,
) -> (TestAppContext, gpui::Entity<MultiBuffer>, Vec<ExcerptRange>) {
    let mut cx = TestAppContext::build(TestDispatcher::new(1), None);
    let sources = (0..source_count)
        .map(|index| {
            let buffer = Buffer::from_text(
                cached_rust_document(SOURCE_BYTES).to_string(),
                BufferConfig::default(),
            )
            .unwrap();
            cx.new(|cx| {
                LanguageBuffer::new(
                    buffer,
                    Some(PathBuf::from(format!("src/source_{index}.rs"))),
                    std::sync::Arc::new(LanguageRegistry::new()),
                    cx,
                )
            })
        })
        .collect::<Vec<_>>();
    let excerpts = cx.read(|cx| {
        sources
            .iter()
            .map(|source| {
                let line_count = source.read(cx).text_snapshot().line_count();
                ExcerptRange::line_range(source.clone(), 0..line_count, cx)
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
                            for excerpt in excerpts {
                                multi_buffer.set_excerpts_for_path(vec![excerpt], cx);
                            }
                        });
                        let snapshot = cx.update_entity(&multi_buffer, |multi_buffer, cx| {
                            multi_buffer.snapshot(cx)
                        });
                        black_box(snapshot.len_bytes());
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// 模拟 diff 将一个源切成大量连续片段后，视口内语法候选的范围投影。
fn source_range_fixture(source_count: usize) -> MultiBufferSnapshot {
    let mut cx = TestAppContext::build(TestDispatcher::new(1), None);
    let multi_buffer = cx.new(MultiBuffer::empty);
    let registry = std::sync::Arc::new(LanguageRegistry::new());
    for index in 0..source_count {
        let source = cx.new(|cx| {
            LanguageBuffer::new(
                Buffer::from_text("let value = 1;\n".repeat(512), BufferConfig::default()).unwrap(),
                Some(PathBuf::from(format!("src/source_{index:03}.rs"))),
                registry.clone(),
                cx,
            )
        });
        let excerpts = cx.read(|cx| {
            (0..512)
                .step_by(4)
                .map(|line| ExcerptRange::line_range(source.clone(), line..line + 4, cx))
                .collect()
        });
        cx.update_entity(&multi_buffer, |multi_buffer, cx| {
            multi_buffer.set_excerpts_for_path(excerpts, cx);
        });
    }
    cx.update_entity(&multi_buffer, |multi_buffer, cx| multi_buffer.snapshot(cx))
}

fn source_range_projection(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_buffer/source_range_projection");
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));
    for source_count in [2, 16, 64] {
        let snapshot = source_range_fixture(source_count);
        for (position, file) in [("first", 0), ("last", source_count - 1)] {
            let output_line = file * 512 + 384;
            let output_offset = snapshot.line_start_byte(Line::new(output_line)).unwrap();
            let source = snapshot.source_at(output_offset).unwrap();
            let start = source.source_offset().get();
            // 同一视口的 24 个候选，范围跨相邻源片段。
            let ranges = (0..24)
                .map(|index| {
                    let start = start + index * 15;
                    ByteOffset::new(start)..ByteOffset::new(start + 90)
                })
                .collect::<Vec<_>>();
            assert!(
                ranges
                    .iter()
                    .all(|range| source.project_range(range.clone()).is_some())
            );
            group.bench_function(format!("{source_count}/{position}"), |b| {
                b.iter(|| {
                    for range in &ranges {
                        black_box(source.project_range(black_box(range.clone())));
                    }
                });
            });
        }
    }
    group.finish();
}

criterion_group!(
    multi_buffer_benches,
    materialize_excerpts,
    source_range_projection
);
criterion_main!(multi_buffer_benches);
