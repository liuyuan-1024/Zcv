//! M19B 搜索 / 替换 benchmark：覆盖 literal / regex 搜索、单次与批量替换、
//! 大文件 / 多匹配场景下的吞吐。
//!
//! 运行：`cargo bench --bench m19_search_replace`。

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use zom_engine::{Buffer, BufferConfig, RegexSearchOptions, SearchOptions};

fn corpus_with_marker(line_count: usize, marker_every: usize, marker: &str) -> String {
    let mut s = String::with_capacity(line_count * 80);
    for i in 0..line_count {
        if i % marker_every == 0 {
            s.push_str(marker);
            s.push(' ');
        }
        s.push_str("alpha beta gamma delta epsilon zeta eta theta\n");
    }
    s
}

fn bench_literal_search(c: &mut Criterion) {
    let text = corpus_with_marker(20_000, 50, "NEEDLE");
    let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();

    c.bench_function("m19_search/literal_default_options", |b| {
        b.iter(|| {
            let result = buffer.search_literal("NEEDLE").unwrap();
            black_box(result.matches().len());
        });
    });

    c.bench_function("m19_search/literal_case_insensitive", |b| {
        let options = SearchOptions::default().with_case_sensitive(false);
        b.iter(|| {
            let result = buffer.search("needle", options.clone()).unwrap();
            black_box(result.matches().len());
        });
    });

    c.bench_function("m19_search/literal_whole_word", |b| {
        let options = SearchOptions::default().with_whole_word(true);
        b.iter(|| {
            let result = buffer.search("alpha", options.clone()).unwrap();
            black_box(result.matches().len());
        });
    });
}

fn bench_regex_search(c: &mut Criterion) {
    let text = corpus_with_marker(20_000, 50, "NEEDLE-42");
    let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();

    c.bench_function("m19_search/regex_simple_anchor", |b| {
        let options = RegexSearchOptions::default();
        b.iter(|| {
            let result = buffer.search_regex(r"NEEDLE-\d+", options.clone()).unwrap();
            black_box(result.matches().len());
        });
    });

    c.bench_function("m19_search/regex_capture_group", |b| {
        let options = RegexSearchOptions::default();
        b.iter(|| {
            let result = buffer
                .search_regex(r"(\b[a-z]{5}\b)", options.clone())
                .unwrap();
            black_box(result.matches().len());
        });
    });
}

fn bench_replace_all_literal(c: &mut Criterion) {
    let text = corpus_with_marker(2_000, 5, "NEEDLE");

    c.bench_function("m19_replace/replace_all_literal_400_matches", |b| {
        b.iter_batched(
            || Buffer::from_text(text.clone(), BufferConfig::default()).unwrap(),
            |mut buffer| {
                let result = buffer.search_literal("NEEDLE").unwrap();
                buffer.replace_all_search_matches(&result, "MATCH").unwrap();
                black_box(buffer.len_chars());
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_replace_all_regex(c: &mut Criterion) {
    let text = corpus_with_marker(2_000, 5, "NEEDLE-99");

    c.bench_function("m19_replace/replace_all_regex_with_capture", |b| {
        b.iter_batched(
            || Buffer::from_text(text.clone(), BufferConfig::default()).unwrap(),
            |mut buffer| {
                let result = buffer
                    .search_regex(r"NEEDLE-(\d+)", RegexSearchOptions::default())
                    .unwrap();
                buffer.replace_all_regex_matches(&result, "M[$1]").unwrap();
                black_box(buffer.len_chars());
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_search_on_long_line(c: &mut Criterion) {
    let mut text = "a".repeat(500_000);
    text.push_str("NEEDLE");
    text.push_str(&"a".repeat(500_000));
    let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();

    c.bench_function("m19_search/literal_in_long_single_line", |b| {
        b.iter(|| {
            let result = buffer.search_literal("NEEDLE").unwrap();
            black_box(result.matches().len());
        });
    });
}

criterion_group!(
    benches,
    bench_literal_search,
    bench_regex_search,
    bench_replace_all_literal,
    bench_replace_all_regex,
    bench_search_on_long_line,
);
criterion_main!(benches);
