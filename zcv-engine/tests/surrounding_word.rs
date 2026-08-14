//! 双击选词范围（surrounding_word）与词内判定（is_inside_word）的集成测试。
//!
//! 语义对齐 Zed 的 `surrounding_word`：目标类别取光标两侧"更词"的一侧，再向左右扫描连续同类字符；换行不参与任何类别的连续性。

mod common;

use common::*;

/// 断言 offset 处双击选中的字符范围（CharOffset）。
fn assert_word(text: &str, offset: usize, expected: (usize, usize)) {
    let buffer = buffer(text);
    assert_eq!(
        buffer.surrounding_word(c(offset)).unwrap(),
        (c(expected.0), c(expected.1)),
        "文本 {text:?} 在 {offset} 处双击选词结果不符"
    );
}

#[test]
fn surrounding_word_selects_the_whole_identifier() {
    // 下划线属于词字符：双击 foo_bar 的任何位置都选中整个标识符。
    assert_word("foo_bar", 0, (0, 7));
    assert_word("foo_bar", 3, (0, 7));
    assert_word("foo_bar", 6, (0, 7));
    // 光标紧贴词尾（右侧是分隔符）时仍选中整个词。
    assert_word("foo_bar!", 7, (0, 7));
    // 光标紧贴词首（左侧是分隔符）时仍选中整个词。
    assert_word("!foo_bar", 1, (1, 8));
    // 美元符属于词字符（对齐 zcv 的 identifier 策略）。
    assert_word("$value", 0, (0, 6));
    assert_word("a$b", 1, (0, 3));
}

#[test]
fn surrounding_word_keeps_cjk_and_emoji_words() {
    // 中文按连续同类别字符整段选中。
    assert_word("世界你好", 1, (0, 4));
    // 中文与 ASCII 字母同为词字符，连续同类因此归属同一词。
    assert_word("foo世界bar", 5, (0, 8));
    // 中文与符号相邻时按符号侧边界切开。
    assert_word("foo-世界", 5, (4, 6));
    // 零宽组合字符随前导字符归属同一词；e\u{301} 是两个 char、一个 grapheme。
    // 词字符连续则同属一词，e\u{301} 与 x 之间无分隔。
    assert_word("e\u{301}!", 0, (0, 2));
    assert_word("e\u{301}x", 0, (0, 3));
}

#[test]
fn surrounding_word_selects_punctuation_runs() {
    // 光标落在标点串中间时，目标类别是符号，选中连续标点。
    assert_word("foo!!!bar", 5, (3, 6));
    // 光标紧贴标点（一侧是词字符）时按词侧扩展，不吞标点。
    assert_word("foo!!!bar", 3, (0, 3));
    // 光标落在符号上（两侧都是词字符）：目标类别是词，向左吃尽词字符，
    // 右侧从光标处字符开始不匹配即停——只选中左侧词（对齐 Zed 算法）。
    assert_word("a.b", 1, (0, 1));
    assert_word("a - b", 2, (2, 3));
    // 光标落在词首（左侧是标点）时选中该词，不吞标点。
    assert_word("foo!!!bar", 6, (6, 9));
}

#[test]
fn surrounding_word_selects_space_runs() {
    // 光标两侧都是空格时目标类别是空白，选中连续空格（不跨行）。
    assert_word("a   b", 2, (1, 4));
    assert_word("a\t\tb", 2, (1, 3));
}

#[test]
fn surrounding_word_does_not_cross_newlines() {
    // 光标在行尾：词只吃本行内容。
    assert_word("foo\nbar", 3, (0, 3));
    // 光标在行首：词只吃本行内容。
    assert_word("foo\nbar", 4, (4, 7));
    // 空格连续不跨行。
    assert_word("a \n b", 2, (1, 2));
}

#[test]
fn surrounding_word_at_empty_line_gap_returns_empty_range() {
    // 光标夹在两个换行之间（空行中间）时没有可选的词。
    let buffer = buffer("abc\n\n  next");
    let gap = "abc\n".chars().count();
    assert_eq!(buffer.surrounding_word(c(gap)).unwrap(), (c(gap), c(gap)));
}

#[test]
fn surrounding_word_rejects_non_grapheme_boundaries() {
    // 组合音标中间不是合法 grapheme 边界，surrounding_word 拒绝查询；
    // is_inside_word 对齐 Zed 不做校验，组合音标是零宽词字符，两侧查询自然为词内。
    let buffer = buffer("e\u{301}x");
    assert!(buffer.surrounding_word(c(1)).is_err());
    assert!(buffer.is_inside_word(c(1)).unwrap());
}

#[test]
fn is_inside_word_detects_word_interior() {
    let buffer = buffer("foo bar");
    assert!(buffer.is_inside_word(c(1)).unwrap());
    assert!(buffer.is_inside_word(c(2)).unwrap());
    // 词首/词尾与分隔符处都不算词内。
    assert!(!buffer.is_inside_word(c(0)).unwrap());
    assert!(!buffer.is_inside_word(c(3)).unwrap());
    assert!(!buffer.is_inside_word(c(4)).unwrap());
    assert!(!buffer.is_inside_word(c(7)).unwrap());
    // 下划线属于词字符，foo_bar 内部仍是词内。
    let underscore = common::buffer("foo_bar");
    assert!(underscore.is_inside_word(c(4)).unwrap());
}
