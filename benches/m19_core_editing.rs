//! M19B 核心编辑操作 benchmark：插入 / 删除 / 替换 / 批量编辑 / Undo / Redo /
//! 多光标编辑 / 坐标转换 / 快照创建。
//!
//! 仅服务性能观察与回归定位，不替代 `tests/` 下的语义契约。运行：
//! `cargo bench --bench m19_core_editing`。

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use zom_engine::{
    Buffer, BufferConfig, CharOffset, Edit, Line, Position, Selection, SelectionSet, TextRange,
    Transaction,
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

fn long_line_text(chars: usize) -> String {
    let mut s = String::with_capacity(chars);
    for i in 0..chars {
        s.push((b'a' + (i as u8 % 26)) as char);
    }
    s
}

fn bench_single_insert(c: &mut Criterion) {
    let text = many_lines_text(20_000, 80);
    let middle = text.chars().count() / 2;

    c.bench_function("m19_core_editing/single_insert_middle", |b| {
        b.iter_batched(
            || Buffer::from_text(text.clone(), BufferConfig::default()).unwrap(),
            |mut buffer| {
                buffer.insert(CharOffset::new(middle), "z").unwrap();
                black_box(buffer.len_chars());
            },
            BatchSize::SmallInput,
        );
    });

    c.bench_function("m19_core_editing/single_insert_long_line", |b| {
        let long = long_line_text(500_000);
        let mid = long.chars().count() / 2;
        b.iter_batched(
            || Buffer::from_text(long.clone(), BufferConfig::default()).unwrap(),
            |mut buffer| {
                buffer.insert(CharOffset::new(mid), "中🙂").unwrap();
                black_box(buffer.len_chars());
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_delete_and_replace(c: &mut Criterion) {
    let text = many_lines_text(20_000, 80);

    c.bench_function("m19_core_editing/delete_range_middle", |b| {
        let total = text.chars().count();
        let start = total / 2;
        let range = TextRange::new(CharOffset::new(start), CharOffset::new(start + 200)).unwrap();
        b.iter_batched(
            || Buffer::from_text(text.clone(), BufferConfig::default()).unwrap(),
            |mut buffer| {
                buffer.delete(range).unwrap();
                black_box(buffer.len_chars());
            },
            BatchSize::SmallInput,
        );
    });

    c.bench_function("m19_core_editing/replace_range_middle", |b| {
        let total = text.chars().count();
        let start = total / 2;
        let range = TextRange::new(CharOffset::new(start), CharOffset::new(start + 200)).unwrap();
        b.iter_batched(
            || Buffer::from_text(text.clone(), BufferConfig::default()).unwrap(),
            |mut buffer| {
                buffer.replace(range, "REPLACE 中文 🙂").unwrap();
                black_box(buffer.len_chars());
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_batch_edit_transaction(c: &mut Criterion) {
    let text = many_lines_text(20_000, 80);

    c.bench_function("m19_core_editing/batch_transaction_64_inserts", |b| {
        let total = text.chars().count();
        b.iter_batched(
            || Buffer::from_text(text.clone(), BufferConfig::default()).unwrap(),
            |mut buffer| {
                let mut edits = Vec::with_capacity(64);
                for i in 0..64 {
                    let off = (i * total) / 70;
                    edits.push(Edit::insert(CharOffset::new(off), "x".to_string()).unwrap());
                }
                let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
                buffer.apply_transaction(tx).unwrap();
                black_box(buffer.len_chars());
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_undo_redo(c: &mut Criterion) {
    let base = many_lines_text(2_000, 60);

    c.bench_function("m19_core_editing/undo_redo_after_many_inserts", |b| {
        b.iter_batched(
            || {
                let mut buffer = Buffer::from_text(base.clone(), BufferConfig::default()).unwrap();
                for i in 0..32 {
                    buffer.insert(CharOffset::new(i * 8), "·").unwrap();
                }
                buffer
            },
            |mut buffer| {
                for _ in 0..32 {
                    buffer.undo().unwrap();
                }
                for _ in 0..32 {
                    buffer.redo().unwrap();
                }
                black_box(buffer.len_chars());
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_multi_caret_insert(c: &mut Criterion) {
    let text = many_lines_text(2_000, 60);

    c.bench_function("m19_core_editing/multi_caret_insert_50_carets", |b| {
        let total = text.chars().count();
        b.iter_batched(
            || Buffer::from_text(text.clone(), BufferConfig::default()).unwrap(),
            |mut buffer| {
                let mut selections = Vec::with_capacity(50);
                for i in 0..50 {
                    let off = (i * total) / 60;
                    selections.push(Selection::new(CharOffset::new(off), CharOffset::new(off)));
                }
                buffer
                    .insert_at_selections(SelectionSet::new(selections), "★")
                    .unwrap();
                black_box(buffer.len_chars());
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_coordinate_conversion(c: &mut Criterion) {
    let text = many_lines_text(20_000, 80);
    let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();
    let line_count = buffer.line_count();
    let queries: Vec<usize> = (0..line_count).step_by(line_count / 64).collect();

    c.bench_function("m19_core_editing/line_to_char_conversion", |b| {
        b.iter(|| {
            for line in &queries {
                black_box(buffer.line_start(Line::new(*line)).unwrap());
            }
        });
    });

    c.bench_function("m19_core_editing/position_to_char_conversion", |b| {
        b.iter(|| {
            for line in &queries {
                let pos = Position::new(Line::new(*line), zom_engine::LogicalColumn::new(0));
                black_box(buffer.position_to_char(pos).unwrap());
            }
        });
    });
}

fn bench_snapshot_creation(c: &mut Criterion) {
    let text = many_lines_text(50_000, 80);
    let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();

    c.bench_function("m19_core_editing/snapshot_clone_50k_lines", |b| {
        b.iter(|| {
            let snap = buffer.snapshot();
            black_box((snap.version(), snap.len_chars(), snap.line_count()));
        });
    });
}

criterion_group!(
    benches,
    bench_single_insert,
    bench_delete_and_replace,
    bench_batch_edit_transaction,
    bench_undo_redo,
    bench_multi_caret_insert,
    bench_coordinate_conversion,
    bench_snapshot_creation,
);
criterion_main!(benches);
