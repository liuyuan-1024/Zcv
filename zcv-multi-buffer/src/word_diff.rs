//! 词级差异：把两段文本按空白 / 单词 / 标点切分，再做 token 级 diff，返回变化片段的字节范围。
//!
//! 只在行级 hunk 内部、新老行数相同且规模受限时使用；范围相对各自输入文本的起点。

use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput, Token};

/// 触发词级 diff 的最大单侧字节长度。
pub(crate) const MAX_WORD_DIFF_BYTES: usize = 512;
/// 触发词级 diff 的最大单侧行数。
pub(crate) const MAX_WORD_DIFF_LINES: usize = 5;

/// 计算两段文本的词级变化范围，返回 (旧侧, 新侧)。
pub(crate) fn word_diff_ranges(
    old_text: &str,
    new_text: &str,
) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let mut input: InternedInput<&str> = InternedInput::default();
    input.update_before(tokenize(old_text));
    input.update_after(tokenize(new_text));

    let mut old_ranges: Vec<Range<usize>> = Vec::new();
    let mut new_ranges: Vec<Range<usize>> = Vec::new();
    diff_internal(&input, &mut |old_byte_range, new_byte_range| {
        if !old_byte_range.is_empty() {
            if let Some(last) = old_ranges.last_mut()
                && last.end >= old_byte_range.start
            {
                last.end = old_byte_range.end;
            } else {
                old_ranges.push(old_byte_range);
            }
        }
        if !new_byte_range.is_empty() {
            if let Some(last) = new_ranges.last_mut()
                && last.end >= new_byte_range.start
            {
                last.end = new_byte_range.end;
            } else {
                new_ranges.push(new_byte_range);
            }
        }
    });
    (old_ranges, new_ranges)
}

fn diff_internal(
    input: &InternedInput<&str>,
    on_change: &mut dyn FnMut(Range<usize>, Range<usize>),
) {
    let mut old_offset = 0;
    let mut new_offset = 0;
    let mut old_token_ix = 0;
    let mut new_token_ix = 0;
    for hunk in Diff::compute(Algorithm::Histogram, input).hunks() {
        old_offset += token_len(
            input,
            &input.before[old_token_ix as usize..hunk.before.start as usize],
        );
        new_offset += token_len(
            input,
            &input.after[new_token_ix as usize..hunk.after.start as usize],
        );
        let old_len = token_len(
            input,
            &input.before[hunk.before.start as usize..hunk.before.end as usize],
        );
        let new_len = token_len(
            input,
            &input.after[hunk.after.start as usize..hunk.after.end as usize],
        );
        let old_byte_range = old_offset..old_offset + old_len;
        let new_byte_range = new_offset..new_offset + new_len;
        old_token_ix = hunk.before.end;
        new_token_ix = hunk.after.end;
        old_offset = old_byte_range.end;
        new_offset = new_byte_range.end;
        on_change(old_byte_range, new_byte_range);
    }
}

fn token_len(input: &InternedInput<&str>, tokens: &[Token]) -> usize {
    tokens
        .iter()
        .map(|token| input.interner[*token].len())
        .sum()
}

fn tokenize(text: &str) -> impl Iterator<Item = &str> {
    let mut chars = text.char_indices();
    let mut prev = None;
    let mut start_ix = 0;
    std::iter::from_fn(move || {
        for (ix, c) in chars.by_ref() {
            let mut token = None;
            let kind = char_kind(c);
            if let Some((prev_char, prev_kind)) = prev
                && (kind != prev_kind || (kind == CharKind::Punctuation && c != prev_char))
            {
                token = Some(&text[start_ix..ix]);
                start_ix = ix;
            }
            prev = Some((c, kind));
            if token.is_some() {
                return token;
            }
        }
        if start_ix < text.len() {
            let token = &text[start_ix..];
            start_ix = text.len();
            return Some(token);
        }
        None
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharKind {
    Whitespace,
    Punctuation,
    Word,
}

fn char_kind(c: char) -> CharKind {
    if c.is_whitespace() {
        CharKind::Whitespace
    } else if c.is_alphanumeric() || c == '_' {
        CharKind::Word
    } else {
        CharKind::Punctuation
    }
}
