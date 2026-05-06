//! M4 存储性能基准：比较 Ropey-backed Buffer 与字符串参考模型在快照、局部编辑和行指标上的成本。
//!
//! 本文件只服务性能观察和回归定位，不定义 public API 契约，也不替代 `tests/m4_storage.rs` 的语义正确性测试。

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use zom_engine::*;

#[derive(Clone)]
struct StringReferenceBuffer {
    text: String,
}

impl StringReferenceBuffer {
    fn new(text: String) -> Self {
        Self { text }
    }

    fn snapshot_clone(&self) -> String {
        self.text.clone()
    }

    fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    fn len_bytes(&self) -> usize {
        self.text.len()
    }

    fn line_count(&self) -> usize {
        // 与 Ropey 的精确行语义不做契约绑定；这里只作为 String reference 性能基线。
        self.text.chars().filter(|ch| *ch == '\n').count() + 1
    }

    fn line_start(&self, line: usize) -> usize {
        if line == 0 {
            return 0;
        }

        let mut current_line = 0usize;

        for (char_idx, ch) in self.text.chars().enumerate() {
            if ch == '\n' {
                current_line += 1;
                if current_line == line {
                    return char_idx + 1;
                }
            }
        }

        self.len_chars()
    }

    fn insert(&mut self, offset: usize, text: &str) {
        let byte = char_to_byte_index(&self.text, offset);
        self.text.insert_str(byte, text);
    }

    fn replace(&mut self, start: usize, end: usize, replacement: &str) {
        let byte_start = char_to_byte_index(&self.text, start);
        let byte_end = char_to_byte_index(&self.text, end);
        self.text.replace_range(byte_start..byte_end, replacement);
    }
}

fn char_to_byte_index(text: &str, char_offset: usize) -> usize {
    if char_offset == text.chars().count() {
        return text.len();
    }

    text.char_indices()
        .nth(char_offset)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(text.len())
}

fn make_long_line_text(chars: usize) -> String {
    "a".repeat(chars)
}

fn make_many_lines_text(lines: usize, line_len: usize) -> String {
    let mut text = String::with_capacity(lines * (line_len + 1));

    for i in 0..lines {
        text.push_str(&format!("{i:06}:"));
        text.push_str(&"x".repeat(line_len.saturating_sub(7)));

        if i + 1 < lines {
            text.push('\n');
        }
    }

    text
}

fn bench_snapshot_clone(c: &mut Criterion) {
    let text = make_many_lines_text(20_000, 80);
    let buffer = Buffer::from_text(text.clone(), BufferConfig::default()).unwrap();
    let reference = StringReferenceBuffer::new(text);

    c.bench_function("m4_snapshot/ropey_buffer_snapshot", |b| {
        b.iter(|| {
            let snapshot = buffer.snapshot();
            black_box((
                snapshot.version(),
                snapshot.len_chars(),
                snapshot.line_count(),
            ));
        });
    });

    c.bench_function("m4_snapshot/string_reference_clone", |b| {
        b.iter(|| {
            let snapshot_text = reference.snapshot_clone();
            black_box((snapshot_text.len(), snapshot_text.chars().count()));
        });
    });
}

fn bench_middle_insert_long_line(c: &mut Criterion) {
    let text = make_long_line_text(500_000);
    let mid = text.chars().count() / 2;

    c.bench_function("m4_edit_middle_long_line/ropey_buffer_insert", |b| {
        b.iter_batched(
            || Buffer::from_text(text.clone(), BufferConfig::default()).unwrap(),
            |mut buffer| {
                buffer.insert(CharOffset::new(mid), "中🙂").unwrap();
                black_box((buffer.len_chars(), buffer.len_bytes()));
            },
            BatchSize::SmallInput,
        );
    });

    c.bench_function("m4_edit_middle_long_line/string_reference_insert", |b| {
        b.iter_batched(
            || StringReferenceBuffer::new(text.clone()),
            |mut reference| {
                reference.insert(mid, "中🙂");
                black_box((reference.len_chars(), reference.len_bytes()));
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_replace_near_start_many_lines(c: &mut Criterion) {
    let text = make_many_lines_text(20_000, 80);
    let start = 128;
    let end = 256;

    c.bench_function(
        "m4_replace_near_start_many_lines/ropey_buffer_replace",
        |b| {
            b.iter_batched(
                || Buffer::from_text(text.clone(), BufferConfig::default()).unwrap(),
                |mut buffer| {
                    buffer
                        .replace(
                            TextRange::new(CharOffset::new(start), CharOffset::new(end)).unwrap(),
                            "replacement 中🙂 text",
                        )
                        .unwrap();
                    black_box((buffer.len_chars(), buffer.line_count()));
                },
                BatchSize::SmallInput,
            );
        },
    );

    c.bench_function(
        "m4_replace_near_start_many_lines/string_reference_replace",
        |b| {
            b.iter_batched(
                || StringReferenceBuffer::new(text.clone()),
                |mut reference| {
                    reference.replace(start, end, "replacement 中🙂 text");
                    black_box((reference.len_chars(), reference.line_count()));
                },
                BatchSize::SmallInput,
            );
        },
    );
}

fn bench_line_metrics(c: &mut Criterion) {
    let lines = 50_000;
    let text = make_many_lines_text(lines, 80);
    let buffer = Buffer::from_text(text.clone(), BufferConfig::default()).unwrap();
    let reference = StringReferenceBuffer::new(text);
    let query_lines = [0usize, 1, 10, 100, 1_000, 10_000, 25_000, 49_999];

    c.bench_function("m4_metrics/ropey_buffer_line_start", |b| {
        b.iter(|| {
            for line in query_lines {
                black_box(buffer.line_start(Line::new(line)).unwrap());
            }
        });
    });

    c.bench_function("m4_metrics/string_reference_line_start", |b| {
        b.iter(|| {
            for line in query_lines {
                black_box(reference.line_start(line));
            }
        });
    });
}

criterion_group!(
    benches,
    bench_snapshot_clone,
    bench_middle_insert_long_line,
    bench_replace_near_start_many_lines,
    bench_line_metrics
);
criterion_main!(benches);
