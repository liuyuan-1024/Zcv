//! Projection 层基线 benchmark：fold 投影建表与视口切片。
//!
//! 三组场景：
//! 1. `Projection::build` 在不同行数 / 不同 fold 密度下的全量重建成本。
//! 2. `Projection::slice_viewport` 固定 50 行视口，验证是否真的 O(viewport)。
//! 3. `logical_to_projected_range_segments` 跨大范围 selection 的段切分。

use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use zom_engine::{
    ApplyOutcome, Buffer, BufferConfig, ByteOffset, FoldSet, Line, LineRange, LogicalPoint,
    LogicalRange, ProjectedLineIndex, ProjectedViewport, Projection,
};

const LINE_COUNTS: &[usize] = &[10_000, 100_000, 500_000];
const VIEWPORT_LINES: usize = 50;

/// 生成 line_count 条每行 ~40 字节的 ASCII 文本。
fn make_lined_text(line_count: usize) -> String {
    const LINE: &str = "the quick brown fox jumps over lazy dog\n";
    let mut s = String::with_capacity(LINE.len() * line_count);
    for _ in 0..line_count {
        s.push_str(LINE);
    }
    s
}

fn make_buffer(line_count: usize) -> Buffer {
    Buffer::from_text(make_lined_text(line_count), BufferConfig::default()).unwrap()
}

/// 在 buffer 中均匀分布 `fold_count` 段 fold，每段折叠 4 行。
/// 折叠相邻行之间留 4 行间隔，避免合并触发 normalize 的合并语义影响 build 路径。
fn make_folds(buffer: &Buffer, fold_count: usize) -> FoldSet {
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(buffer.version());
    if fold_count == 0 {
        return folds;
    }
    let total_lines = buffer.line_count();
    // 每个 fold 占 4 行 + 4 行间隔 = 8 行/槽
    let stride = (total_lines / fold_count).max(8);
    for i in 0..fold_count {
        let start = i * stride + 1; // 跳过 line 0 给 anchor 留位置
        let end = start + 4;
        if end >= total_lines {
            break;
        }
        let range = LineRange::new(Line::new(start), Line::new(end)).unwrap();
        folds.fold_lines(&snapshot, range).unwrap();
    }
    folds
}

fn bench_projection_build_no_folds(c: &mut Criterion) {
    let mut group = c.benchmark_group("projection_build_no_folds");
    for &lines in LINE_COUNTS {
        let buffer = make_buffer(lines);
        let snapshot = buffer.snapshot();
        let folds = FoldSet::new(buffer.version());
        group.bench_function(BenchmarkId::from_parameter(lines), |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let t0 = Instant::now();
                    let proj = Projection::build(&snapshot, &folds).unwrap();
                    total += t0.elapsed();
                    drop(proj);
                }
                total
            });
        });
    }
    group.finish();
}

fn bench_projection_build_with_folds(c: &mut Criterion) {
    let mut group = c.benchmark_group("projection_build_with_folds_100k_lines");
    let buffer = make_buffer(100_000);
    let snapshot = buffer.snapshot();
    for &fold_count in &[0usize, 100, 1_000, 10_000] {
        let folds = make_folds(&buffer, fold_count);
        group.bench_function(BenchmarkId::from_parameter(fold_count), |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let t0 = Instant::now();
                    let proj = Projection::build(&snapshot, &folds).unwrap();
                    total += t0.elapsed();
                    drop(proj);
                }
                total
            });
        });
    }
    group.finish();
}

fn bench_slice_viewport_50rows(c: &mut Criterion) {
    let mut group = c.benchmark_group("projection_slice_viewport_50rows");
    for &lines in LINE_COUNTS {
        let buffer = make_buffer(lines);
        let snapshot = buffer.snapshot();
        let folds = FoldSet::new(buffer.version());
        let projection = Projection::build(&snapshot, &folds).unwrap();
        // 视口位于文件中部
        let start_row = projection.line_count() / 2;
        let viewport = ProjectedViewport::new(ProjectedLineIndex::new(start_row), VIEWPORT_LINES);
        group.bench_function(BenchmarkId::from_parameter(lines), |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let t0 = Instant::now();
                    let slice = projection.slice_viewport(&snapshot, viewport).unwrap();
                    total += t0.elapsed();
                    drop(slice);
                }
                total
            });
        });
    }
    group.finish();
}

fn bench_range_segments(c: &mut Criterion) {
    let mut group = c.benchmark_group("projection_range_segments_full_range");
    // 100k 行 buffer，分别测：无 fold / 100 fold / 1000 fold；范围跨整个 buffer。
    let buffer = make_buffer(100_000);
    let snapshot = buffer.snapshot();
    for &fold_count in &[0usize, 100, 1_000] {
        let folds = make_folds(&buffer, fold_count);
        let projection = Projection::build(&snapshot, &folds).unwrap();
        let range = LogicalRange::new(
            LogicalPoint::line_start(Line::new(0)),
            LogicalPoint::line_start(Line::new(99_000)),
        )
        .unwrap();
        group.bench_function(BenchmarkId::from_parameter(fold_count), |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let t0 = Instant::now();
                    let segs = projection
                        .logical_to_projected_range_segments(range)
                        .unwrap();
                    total += t0.elapsed();
                    drop(segs);
                }
                total
            });
        });
    }
    group.finish();
}

/// 行内字节编辑：在文件中部一行的中间插入一个 ASCII 字符。
/// 不改变行数、不影响 fold 结构 → 期望 `apply_delta` 命中 `Compatible` 路径，O(1)。
fn bench_apply_delta_inline_edit(c: &mut Criterion) {
    let mut group = c.benchmark_group("projection_apply_delta_inline");
    for &fold_count in &[0usize, 100, 1_000, 10_000] {
        group.bench_function(BenchmarkId::from_parameter(fold_count), |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let mut buffer = make_buffer(100_000);
                    let snapshot = buffer.snapshot();
                    let mut folds = make_folds(&buffer, fold_count);
                    let mut projection = Projection::build(&snapshot, &folds).unwrap();

                    // 找一个"行中间"的字节位置：取第 50000 行起点 + 5。
                    let mid_line_start = buffer.line_start_byte(Line::new(50_000)).unwrap();
                    let edit_offset = ByteOffset::new(mid_line_start.get() + 5);

                    buffer.insert(edit_offset, "x").unwrap();
                    let event = buffer.last_delta_event().unwrap().clone();
                    let new_snapshot = buffer.snapshot();
                    folds
                        .update_through_delta_event(&event, &new_snapshot)
                        .unwrap();

                    let t0 = Instant::now();
                    let outcome = projection
                        .apply_delta(&new_snapshot, &folds, &event)
                        .unwrap();
                    total += t0.elapsed();

                    assert!(matches!(outcome, ApplyOutcome::Compatible));
                    drop(projection);
                }
                total
            });
        });
    }
    group.finish();
}

/// Enter 风格编辑：插入一个换行 → 行数变化 → 期望 `apply_delta` 走 `Rebuilt` 路径。
/// 验证 Rebuilt 与"先 build_with_folds"基线的实际开销一致。
fn bench_apply_delta_newline_edit(c: &mut Criterion) {
    let mut group = c.benchmark_group("projection_apply_delta_newline");
    for &fold_count in &[0usize, 100, 1_000, 10_000] {
        group.bench_function(BenchmarkId::from_parameter(fold_count), |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let mut buffer = make_buffer(100_000);
                    let snapshot = buffer.snapshot();
                    let mut folds = make_folds(&buffer, fold_count);
                    let mut projection = Projection::build(&snapshot, &folds).unwrap();

                    let mid_line_start = buffer.line_start_byte(Line::new(50_000)).unwrap();
                    let edit_offset = ByteOffset::new(mid_line_start.get() + 5);

                    buffer.insert(edit_offset, "\n").unwrap();
                    let event = buffer.last_delta_event().unwrap().clone();
                    let new_snapshot = buffer.snapshot();
                    folds
                        .update_through_delta_event(&event, &new_snapshot)
                        .unwrap();

                    let t0 = Instant::now();
                    let outcome = projection
                        .apply_delta(&new_snapshot, &folds, &event)
                        .unwrap();
                    total += t0.elapsed();

                    assert!(matches!(outcome, ApplyOutcome::Rebuilt));
                    drop(projection);
                }
                total
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_projection_build_no_folds,
    bench_projection_build_with_folds,
    bench_slice_viewport_50rows,
    bench_range_segments,
    bench_apply_delta_inline_edit,
    bench_apply_delta_newline_edit,
);
criterion_main!(benches);
