use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
mod common;

use common::cached_rust_document;
use zcv_project::{SearchQuery, SearchQueryResult, regex_replacements_in_text};
use zcv_text::{
    Buffer, BufferConfig, ByteOffset, Edit, Line, TransactionMetadata, WordBoundaryPolicy,
};

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
        let literal_query = SearchQuery {
            query: "render_document".to_string(),
            ..Default::default()
        }
        .prepare()
        .unwrap();
        group.bench_with_input(
            BenchmarkId::new("literal", byte_len),
            &snapshot,
            |b, snapshot| {
                b.iter(|| {
                    black_box(
                        literal_query
                            .search(snapshot, WordBoundaryPolicy::default())
                            .unwrap()
                            .matches()
                            .len(),
                    )
                });
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
                b.iter(|| {
                    black_box(
                        query
                            .search(snapshot, WordBoundaryPolicy::default())
                            .unwrap()
                            .matches()
                            .len(),
                    )
                });
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

/// 长行：单行超长文本的创建与坐标换算，覆盖横向滚动与显示列定位的数据源。
fn long_lines(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_buffer/long_line");
    let line = "abcdefghij".repeat(1024);
    let rows = 64;
    let text = (0..rows).map(|_| format!("{line}\n")).collect::<String>();
    let snapshot = Buffer::from_text(text.clone(), BufferConfig::default())
        .expect("长行文档应能创建")
        .snapshot();
    group.throughput(Throughput::Bytes(text.len() as u64));
    group.bench_function("create", |b| {
        b.iter(|| {
            Buffer::from_text(black_box(text.clone()), BufferConfig::default())
                .expect("长行文档应能创建")
        });
    });

    let positions = [0, rows / 2, rows - 1].map(|row| {
        snapshot
            .byte_to_position(ByteOffset::new(row * (line.len() + 1) + line.len() / 2))
            .expect("长行中部应有位置")
    });
    group.bench_function("position_roundtrip", |b| {
        b.iter(|| {
            for position in positions {
                black_box(snapshot.position_to_byte(position).expect("位置应可还原"));
            }
        });
    });
    group.finish();
}

/// 搜索替换：literal 与 regex 两路，测量查找全部匹配并生成替换结果。
fn replace_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_buffer/replace_all");

    for size in DOCUMENT_SIZES {
        let text = cached_rust_document(size);
        let byte_len = text.len();
        let snapshot = Buffer::from_text(text.to_string(), BufferConfig::default())
            .expect("替换基准应能创建 Buffer")
            .snapshot();

        let literal = SearchQuery {
            query: "render_document".to_string(),
            ..Default::default()
        }
        .prepare()
        .expect("literal 查询应能预编译");
        group.bench_with_input(
            BenchmarkId::new("literal", byte_len),
            &(literal, snapshot.clone()),
            |b, (query, snapshot)| {
                b.iter(|| {
                    let result = query
                        .search(snapshot, WordBoundaryPolicy::default())
                        .expect("搜索应成功");
                    black_box(result.matches().len());
                });
            },
        );

        let regex = SearchQuery {
            query: "render_document\\(index: usize\\)".to_string(),
            regex: true,
            ..Default::default()
        }
        .prepare()
        .expect("regex 查询应能预编译");
        group.bench_with_input(
            BenchmarkId::new("regex", byte_len),
            &(regex, snapshot),
            |b, (query, snapshot)| {
                b.iter(|| {
                    let result = query
                        .search(snapshot, WordBoundaryPolicy::default())
                        .expect("搜索应成功");
                    if let SearchQueryResult::Regex(result) = &result {
                        let count = regex_replacements_in_text(
                            snapshot,
                            result,
                            "render_document(index: u64)",
                        )
                        .expect("替换迭代器应能建立")
                        .count();
                        black_box(count);
                    }
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    text_buffer_benches,
    buffers,
    editing,
    searches,
    coordinates,
    long_lines,
    replace_all
);
criterion_main!(text_buffer_benches);
