//! 增量按行宽度摘要：`Snapshot::text_summary_for_range` 必须与逐行扫描一致，并覆盖插入/删除行、跨行替换、结尾换行、CRLF、undo/redo、大事务与派生快照。

use zcv_text::{
    Buffer, BufferConfig, ByteOffset, Edit, LargeTransactionPolicy, Snapshot, TextRange,
    TextSummary, TransactionMetadata,
};

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).expect("测试区间必须满足 start <= end")
}

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).expect("Buffer 必须能创建")
}

/// 独立参考实现：直接扫描文本，按稳定语义计算多维摘要。
fn scan_summary(text: &str) -> TextSummary {
    let mut summary = TextSummary {
        len: text.len(),
        ..TextSummary::default()
    };
    for character in text.chars() {
        summary.chars += 1;
        summary.len_utf16 += character.len_utf16();
        if character == '\n' {
            summary.lines += 1;
            summary.last_line_chars = 0;
        } else {
            summary.last_line_chars += 1;
        }
        if summary.lines == 0 {
            summary.first_line_chars = summary.last_line_chars;
        }
        if summary.last_line_chars > summary.longest_row_chars {
            summary.longest_row = summary.lines;
            summary.longest_row_chars = summary.last_line_chars;
        }
    }
    summary
}

/// 合法编辑边界：字符边界，且不落在 CRLF 的 \r 与 \n 之间（该位置也构成 grapheme 内部）。
fn char_boundaries(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut boundaries: Vec<usize> = text
        .char_indices()
        .map(|(index, _)| index)
        .filter(|index| !(*index > 0 && bytes[*index - 1] == b'\r' && bytes[*index] == b'\n'))
        .collect();
    boundaries.push(text.len());
    boundaries
}

fn snapshot_text(snapshot: &Snapshot) -> String {
    snapshot
        .slice_text(range(0, snapshot.len_bytes().get()))
        .expect("全文范围必须合法")
        .as_str()
        .to_string()
}

/// 校验整段以及所有字符边界子区间的摘要都等于同文本的逐行扫描。
fn assert_summary_matches_scan(buffer: &Buffer, text: &str) {
    let snapshot = buffer.snapshot();
    assert_eq!(
        snapshot
            .text_summary_for_range(range(0, text.len()))
            .expect("整段范围必须合法"),
        scan_summary(text),
        "整段摘要必须等于逐行扫描"
    );

    let boundaries = char_boundaries(text);
    for &start in &boundaries {
        for &end in &boundaries {
            if end < start {
                continue;
            }
            assert_eq!(
                snapshot
                    .text_summary_for_range(range(start, end))
                    .expect("字符边界区间必须合法"),
                scan_summary(&text[start..end]),
                "区间 {start}..{end} 的摘要必须等于逐行扫描"
            );
        }
    }
}

#[test]
fn text_summary_for_range_matches_zed_semantics() {
    let buffer = buffer("ab\nefg\nhklm\nnopqrs\ntuvwxyz");
    let snapshot = buffer.snapshot();
    let checks = [
        (
            0..2,
            TextSummary {
                len: 2,
                chars: 2,
                len_utf16: 2,
                lines: 0,
                first_line_chars: 2,
                last_line_chars: 2,
                longest_row: 0,
                longest_row_chars: 2,
            },
        ),
        (
            1..3,
            TextSummary {
                len: 2,
                chars: 2,
                len_utf16: 2,
                lines: 1,
                first_line_chars: 1,
                last_line_chars: 0,
                longest_row: 0,
                longest_row_chars: 1,
            },
        ),
        (
            1..12,
            TextSummary {
                len: 11,
                chars: 11,
                len_utf16: 11,
                lines: 3,
                first_line_chars: 1,
                last_line_chars: 0,
                longest_row: 2,
                longest_row_chars: 4,
            },
        ),
        (
            0..20,
            TextSummary {
                len: 20,
                chars: 20,
                len_utf16: 20,
                lines: 4,
                first_line_chars: 2,
                last_line_chars: 1,
                longest_row: 3,
                longest_row_chars: 6,
            },
        ),
        (
            0..22,
            TextSummary {
                len: 22,
                chars: 22,
                len_utf16: 22,
                lines: 4,
                first_line_chars: 2,
                last_line_chars: 3,
                longest_row: 3,
                longest_row_chars: 6,
            },
        ),
        (
            7..22,
            TextSummary {
                len: 15,
                chars: 15,
                len_utf16: 15,
                lines: 2,
                first_line_chars: 4,
                last_line_chars: 3,
                longest_row: 1,
                longest_row_chars: 6,
            },
        ),
    ];

    for (spec, expected) in checks {
        assert_eq!(
            snapshot
                .text_summary_for_range(range(spec.start, spec.end))
                .expect("用例区间必须合法"),
            expected,
            "区间 {spec:?}"
        );
    }
}

#[test]
fn targeted_edits_keep_summary_equal_to_scan() {
    let mut buffer = buffer("alpha\nbravo\ncharlie\n");
    assert_summary_matches_scan(&buffer, "alpha\nbravo\ncharlie\n");

    // 在行尾插入换行，拆出空行。
    buffer
        .edit(
            [Edit::replace(range(5, 6), "\n\n")],
            TransactionMetadata::default(),
        )
        .expect("插入换行必须合法");
    assert_eq!(
        snapshot_text(&buffer.snapshot()),
        "alpha\n\nbravo\ncharlie\n"
    );
    assert_summary_matches_scan(&buffer, "alpha\n\nbravo\ncharlie\n");

    // 删除整行（含行终止符）。
    buffer
        .edit(
            [Edit::replace(range(7, 13), "")],
            TransactionMetadata::default(),
        )
        .expect("删除整行必须合法");
    assert_eq!(snapshot_text(&buffer.snapshot()), "alpha\n\ncharlie\n");
    assert_summary_matches_scan(&buffer, "alpha\n\ncharlie\n");

    // 跨行替换：把两行合并成一行。
    buffer
        .edit(
            [Edit::replace(range(0, 7), "x\n")],
            TransactionMetadata::default(),
        )
        .expect("跨行替换必须合法");
    assert_eq!(snapshot_text(&buffer.snapshot()), "x\ncharlie\n");
    assert_summary_matches_scan(&buffer, "x\ncharlie\n");

    // 去掉结尾换行，最后一行变为不完整行。
    let len = buffer.len_bytes().get();
    buffer
        .edit(
            [Edit::replace(range(len - 1, len), "")],
            TransactionMetadata::default(),
        )
        .expect("去掉结尾换行必须合法");
    assert_eq!(snapshot_text(&buffer.snapshot()), "x\ncharlie");
    assert_summary_matches_scan(&buffer, "x\ncharlie");
}

#[test]
fn trailing_newline_crlf_and_multibyte_summaries_are_stable() {
    for text in [
        "",
        "a",
        "a\n",
        "\n",
        "a\r\nb",
        "a\r\n",
        "\r\n",
        "\r\na\r\n\r\n",
        "中\n🙂\r\nb",
    ] {
        let summary = buffer(text)
            .snapshot()
            .text_summary_for_range(range(0, text.len()))
            .expect("整段范围必须合法");
        assert_eq!(summary, scan_summary(text), "文本 {text:?}");
    }
}

#[test]
fn random_edits_keep_summary_equal_to_scan() {
    let mut rng = Lcg::new(0x9E37_79B9_7F4A_7C15);
    let mut text = "ab\ncde\r\nf".to_string();
    let mut buffer = buffer(&text);
    let replacements = ["", "\n", "\r\n", "x", "中", "ab\n", "\n\n", "hello", "🙂"];

    for _ in 0..250 {
        let boundaries = char_boundaries(&text);
        let start_index = rng.next_usize(boundaries.len());
        let end_index = start_index + rng.next_usize(boundaries.len() - start_index);
        let start = boundaries[start_index];
        let end = boundaries[end_index];

        let mut replacement = replacements[rng.next_usize(replacements.len())];
        // 让文本长度在有限范围内震荡，保证全区间对照仍可接受。
        if text.len() >= 40 && replacement.len() > end - start {
            replacement = "";
        }
        if text.len() <= 6 && replacement.is_empty() && start == end {
            replacement = "z";
        }
        if start == end && replacement.is_empty() {
            continue;
        }

        buffer
            .edit(
                [Edit::replace(range(start, end), replacement)],
                TransactionMetadata::default(),
            )
            .expect("生成的编辑必须合法");
        text.replace_range(start..end, replacement);

        assert_summary_matches_scan(&buffer, &text);
    }
}

#[test]
fn undo_redo_keep_summary_equal_to_scan() {
    let mut buffer = buffer("ab\ncd");
    buffer
        .edit(
            [Edit::replace(range(0, 1), "XY\n")],
            TransactionMetadata::default(),
        )
        .unwrap();
    assert_summary_matches_scan(&buffer, &snapshot_text(&buffer.snapshot()));

    buffer
        .edit(
            [Edit::replace(range(3, 4), "")],
            TransactionMetadata::default(),
        )
        .unwrap();
    assert_summary_matches_scan(&buffer, &snapshot_text(&buffer.snapshot()));

    while buffer.can_undo() {
        buffer.undo().expect("undo 必须成功");
        assert_summary_matches_scan(&buffer, &snapshot_text(&buffer.snapshot()));
    }
    while buffer.can_redo() {
        buffer.redo().expect("redo 必须成功");
        assert_summary_matches_scan(&buffer, &snapshot_text(&buffer.snapshot()));
    }
}

#[test]
fn skipped_history_large_transaction_still_advances_summary() {
    let mut config = BufferConfig::default();
    config.large_file.large_transaction_threshold_bytes = 4;
    config.large_file.large_transaction_policy = LargeTransactionPolicy::SkipHistory;
    let mut buffer = Buffer::from_text("a\nb\n".to_string(), config).expect("Buffer 必须能创建");

    buffer
        .edit(
            [Edit::replace(range(0, 3), "wide\nline\n")],
            TransactionMetadata::default(),
        )
        .expect("大事务必须提交文本");

    assert!(!buffer.can_undo(), "大事务必须丢弃历史");
    let text = snapshot_text(&buffer.snapshot());
    assert_eq!(text, "wide\nline\n\n");
    assert_summary_matches_scan(&buffer, &text);
}

#[test]
fn derived_snapshot_and_fast_forward_expose_the_same_summary() {
    let mut buffer = buffer("one\ntwo\nthree");
    let edited = buffer
        .snapshot_with_edits([Edit::replace(range(4, 7), "2\n")])
        .unwrap();

    let derived_text = snapshot_text(edited.snapshot());
    assert_eq!(derived_text, "one\n2\n\nthree");
    assert_eq!(
        edited
            .snapshot()
            .text_summary_for_range(range(0, derived_text.len()))
            .unwrap(),
        scan_summary(&derived_text)
    );
    assert_eq!(
        buffer
            .snapshot()
            .text_summary_for_range(range(0, 13))
            .unwrap(),
        scan_summary("one\ntwo\nthree")
    );

    buffer.fast_forward(edited).unwrap();
    let installed_text = snapshot_text(&buffer.snapshot());
    assert_eq!(installed_text, derived_text);
    assert_summary_matches_scan(&buffer, &installed_text);
}

#[test]
fn text_summary_for_range_rejects_invalid_ranges() {
    let buffer = buffer("a中b");
    let snapshot = buffer.snapshot();

    assert!(snapshot.text_summary_for_range(range(0, 99)).is_err());
    // '中' 占据字节 1..4，字节 2 不是字符边界。
    assert!(snapshot.text_summary_for_range(range(0, 2)).is_err());
}

/// 确定性伪随机源；测试只要求可复现的编辑序列，不引入额外依赖。
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % bound.max(1)
    }
}
