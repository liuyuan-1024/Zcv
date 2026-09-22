//! 文本差异：把旧文本变为新文本的最小编辑段（行级 diff + 行内词级细化）。
//!
//! `Buffer::replace_text` 等外部文本更新场景用 diff 生成真实的 old -> new 编辑；
//! 替换文本与旧区间一起产出，消费方不再用区间切分原文。
//! 行级与词级 diff 都基于 imara-diff，与 Zed 的 `text_diff` 使用同一套 token 化与差异原语。

use std::ops::Range;

use imara_diff::{InternedInput, sources::lines};

use crate::{
    Edit,
    types::{ByteOffset, TextRange},
    word_diff::{MAX_WORD_DIFF_BYTES, MAX_WORD_DIFF_LINES, for_each_token_change, tokenize},
};

/// 把 `old` 变为 `new` 的净变化编辑序列；匹配区域不产生编辑。
///
/// 先做行级 diff，再对规模受限且两侧都非空的替换块做行内词级 diff（对齐 Zed `text_diff`）。
/// 契约：产出的每个编辑区间都落在旧文本的 UTF-8 字符边界上，替换文本由本函数切片。
pub fn diff_edits(old: &str, new: &str) -> Vec<Edit> {
    let mut edits = Vec::new();
    let mut hunk_input = InternedInput::default();
    let input = InternedInput::new(lines(old), lines(new));
    for_each_token_change(&input, &mut |old_range, new_range, old_rows, new_rows| {
        if should_refine_inline(&old_rows, &old_range, &new_rows, &new_range) {
            let old_offset = old_range.start;
            let new_offset = new_range.start;
            hunk_input.clear();
            hunk_input.update_before(tokenize(&old[old_range.clone()]));
            hunk_input.update_after(tokenize(&new[new_range.clone()]));
            for_each_token_change(&hunk_input, &mut |inline_old, inline_new, _, _| {
                push_edit(
                    &mut edits,
                    old_offset + inline_old.start,
                    old_offset + inline_old.end,
                    new_offset + inline_new.start,
                    new_offset + inline_new.end,
                    new,
                );
            });
        } else {
            push_edit(
                &mut edits,
                old_range.start,
                old_range.end,
                new_range.start,
                new_range.end,
                new,
            );
        }
    });
    edits
}

/// 行级替换块是否细化到词级：两侧都非空，且字节数与行数都在阈值内（对齐 Zed）。
fn should_refine_inline(
    old_rows: &Range<u32>,
    old_bytes: &Range<usize>,
    new_rows: &Range<u32>,
    new_bytes: &Range<usize>,
) -> bool {
    !old_bytes.is_empty()
        && !new_bytes.is_empty()
        && old_bytes.len() <= MAX_WORD_DIFF_BYTES
        && new_bytes.len() <= MAX_WORD_DIFF_BYTES
        && old_rows.len() <= MAX_WORD_DIFF_LINES
        && new_rows.len() <= MAX_WORD_DIFF_LINES
}

/// 记录一条替换编辑；替换文本在此处按字符边界切片，消费方不再切原文。
fn push_edit(
    edits: &mut Vec<Edit>,
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
    new: &str,
) {
    if old_start == old_end && new_start == new_end {
        return;
    }
    edits.push(Edit::new(
        TextRange::new(ByteOffset::new(old_start), ByteOffset::new(old_end))
            .expect("diff 产出的旧区间必须有序"),
        &new[new_start..new_end],
    ));
}

#[cfg(test)]
#[path = "test/diff_tests.rs"]
mod tests;
