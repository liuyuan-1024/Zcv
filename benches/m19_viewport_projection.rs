//! M19B viewport / projection benchmark：覆盖 viewport slicing、projection 构建
//! 与 logical / projected 双向映射，重点观察折叠后视口读取在大文件下的延迟。
//!
//! 运行：`cargo bench --bench m19_viewport_projection`。

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use zom_engine::{
    Buffer, BufferConfig, FoldSet, Line, LineRange, LogicalColumn, LogicalPoint,
    ProjectedLineIndex, ProjectedViewport, Projection, Viewport,
};

fn many_lines_text(line_count: usize, line_width: usize) -> String {
    let mut s = String::with_capacity(line_count * (line_width + 1));
    for i in 0..line_count {
        for _ in 0..line_width {
            s.push((b'a' + (i as u8 % 26)) as char);
        }
        s.push('\n');
    }
    s
}

fn bench_viewport_slice_no_fold(c: &mut Criterion) {
    let text = many_lines_text(50_000, 80);
    let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();
    let snapshot = buffer.snapshot();
    let viewport = Viewport::new(Line::new(20_000), 60).with_max_line_chars(120);

    c.bench_function("m19_viewport/slice_60_lines_50k_buffer", |b| {
        b.iter(|| {
            let slice = snapshot.slice_viewport(viewport).unwrap();
            black_box(slice.lines().len());
        });
    });
}

fn bench_viewport_slice_long_line(c: &mut Criterion) {
    let mut text = String::new();
    for _ in 0..1_000 {
        text.push_str(&"x".repeat(2_000));
        text.push('\n');
    }
    let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();
    let snapshot = buffer.snapshot();
    let viewport = Viewport::new(Line::new(0), 60).with_max_line_chars(200);

    c.bench_function("m19_viewport/slice_long_line_truncated", |b| {
        b.iter(|| {
            let slice = snapshot.slice_viewport(viewport).unwrap();
            black_box(slice.lines().len());
        });
    });
}

fn bench_projection_build_no_fold(c: &mut Criterion) {
    let text = many_lines_text(20_000, 80);
    let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();
    let snapshot = buffer.snapshot();
    let folds = FoldSet::new(buffer.version());

    c.bench_function("m19_projection/build_20k_lines_no_folds", |b| {
        b.iter(|| {
            let projection = Projection::build(&snapshot, &folds).unwrap();
            black_box(projection.line_count());
        });
    });
}

fn bench_projection_build_with_folds(c: &mut Criterion) {
    let text = many_lines_text(20_000, 80);
    let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(buffer.version());
    for i in 0..200 {
        let start = (i as usize) * 80;
        let end = start + 30;
        let _ = folds.fold_lines(
            &buffer,
            LineRange::new(Line::new(start), Line::new(end.min(20_000))).unwrap(),
        );
    }

    c.bench_function("m19_projection/build_20k_lines_200_folds", |b| {
        b.iter(|| {
            let projection = Projection::build(&snapshot, &folds).unwrap();
            black_box(projection.line_count());
        });
    });
}

fn bench_projection_logical_to_projected(c: &mut Criterion) {
    let text = many_lines_text(20_000, 80);
    let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(buffer.version());
    for i in 0..50 {
        let start = (i as usize) * 200;
        let end = start + 50;
        let _ = folds.fold_lines(
            &buffer,
            LineRange::new(Line::new(start), Line::new(end.min(20_000))).unwrap(),
        );
    }
    let projection = Projection::build(&snapshot, &folds).unwrap();
    let queries: Vec<Line> = (0..50).map(|i| Line::new((i as usize) * 400)).collect();

    c.bench_function("m19_projection/logical_to_projected_50_queries", |b| {
        b.iter(|| {
            for line in &queries {
                let point = LogicalPoint::new(*line, LogicalColumn::new(0));
                black_box(projection.logical_to_projected_point(point).unwrap());
            }
        });
    });
}

fn bench_projection_slice_viewport_with_folds(c: &mut Criterion) {
    let text = many_lines_text(20_000, 80);
    let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();
    let snapshot = buffer.snapshot();
    let mut folds = FoldSet::new(buffer.version());
    for i in 0..100 {
        let start = (i as usize) * 100;
        let end = start + 30;
        let _ = folds.fold_lines(
            &buffer,
            LineRange::new(Line::new(start), Line::new(end.min(20_000))).unwrap(),
        );
    }
    let projection = Projection::build(&snapshot, &folds).unwrap();
    let viewport =
        ProjectedViewport::new(ProjectedLineIndex::new(5_000), 60).with_max_line_chars(120);

    c.bench_function("m19_projection/slice_viewport_with_folds", |b| {
        b.iter(|| {
            let slice = projection.slice_viewport(&snapshot, viewport).unwrap();
            black_box(slice.rows().len());
        });
    });
}

criterion_group!(
    benches,
    bench_viewport_slice_no_fold,
    bench_viewport_slice_long_line,
    bench_projection_build_no_fold,
    bench_projection_build_with_folds,
    bench_projection_logical_to_projected,
    bench_projection_slice_viewport_with_folds,
);
criterion_main!(benches);
