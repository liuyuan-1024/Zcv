//! Buffer 编辑路径基线 benchmark。
//!
//! 覆盖最常回归的批量 edit 插入与删除路径。
//! 文本规模选 1MiB 与 10MiB；纯 ASCII，任意字节位置都是合法 UTF-8 边界。

use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use ropey::Rope;
use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, TextRange, TransactionMetadata};

const SIZES: &[usize] = &[1 << 20, 10 << 20];

fn make_text(target_bytes: usize) -> String {
    const LINE: &str = "the quick brown fox jumps over the lazy dog 0123456789 abcdefghij klmnop\n";
    let mut s = String::with_capacity(target_bytes + LINE.len());
    while s.len() < target_bytes {
        s.push_str(LINE);
    }
    s.truncate(target_bytes);
    s
}

fn fresh_buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_owned(), BufferConfig::default()).unwrap()
}

/// 用 `iter_custom` 手动控制计时器：setup 与 drop 都明确排除在测量之外。
/// 避免 10 MiB rope drop 的 ~300µs 噪声把 sub-µs 编辑操作淹没。
fn bench_insert_middle(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_insert_middle");
    for &size in SIZES {
        let text = make_text(size);
        let mid = ByteOffset::new(size / 2);
        group.bench_function(BenchmarkId::from_parameter(size), |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let mut buf = fresh_buffer(&text);
                    let start = Instant::now();
                    buf.edit(
                        [Edit::insert(mid, "x").unwrap()],
                        TransactionMetadata::default(),
                    )
                    .unwrap();
                    total += start.elapsed();
                    drop(buf);
                }
                total
            });
        });
    }
    group.finish();
}

fn bench_delete_middle(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_delete_middle_64b");
    for &size in SIZES {
        let text = make_text(size);
        let start_off = ByteOffset::new(size / 2);
        let end_off = ByteOffset::new(size / 2 + 64);
        let range = TextRange::new(start_off, end_off).unwrap();
        group.bench_function(BenchmarkId::from_parameter(size), |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let mut buf = fresh_buffer(&text);
                    let t0 = Instant::now();
                    buf.edit([Edit::delete(range)], TransactionMetadata::default())
                        .unwrap();
                    total += t0.elapsed();
                    drop(buf);
                }
                total
            });
        });
    }
    group.finish();
}

/// 对照组：裸 ropey `Rope::insert` 不经任何 Buffer 管线。
/// 用于把 engine 管线开销与 ropey 自身开销分离开。
fn control_pure_rope_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("control_pure_rope_insert");
    for &size in SIZES {
        let text = make_text(size);
        let mid_byte = size / 2;
        group.bench_function(BenchmarkId::from_parameter(size), |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let mut rope = Rope::from_str(&text);
                    let t0 = Instant::now();
                    let char_idx = rope.byte_to_char(mid_byte);
                    rope.insert(char_idx, "x");
                    total += t0.elapsed();
                    drop(rope);
                }
                total
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_insert_middle,
    bench_delete_middle,
    control_pure_rope_insert,
);
criterion_main!(benches);
