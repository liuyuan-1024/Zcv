use super::word_diff_ranges;

/// 词级 diff 按空白 / 单词 / 标点切分：只有真正变化的词进入范围，相同文本无范围。
#[test]
fn word_diff_ranges_split_words_and_punctuation() {
    let (old, new) = word_diff_ranges(
        "let x = 1;
",
        "let x = 2;
",
    );
    assert_eq!(old, vec![8..9], "旧侧只应包含变化的数字");
    assert_eq!(new, vec![8..9], "新侧只应包含变化的数字");

    let (old, new) = word_diff_ranges("a b c", "a X c");
    assert_eq!(old, vec![2..3]);
    assert_eq!(new, vec![2..3]);

    assert_eq!(
        word_diff_ranges(
            "same
", "same
"
        ),
        (Vec::new(), Vec::new()),
        "相同文本不应产生词级范围"
    );
}
