//! 文本差异：把旧文本变为新文本的最小编辑段（行级 LCS + 字符级细化）。
//!
//! reload 等外部整体替换场景用 diff 生成真实的 old -> new 坐标映射 patch，使选区 / 折叠端点跟随外部变更后的具体位置，而不是整体替换塌缩。

use crate::text_changes::{PatchEdit, TextPatch};

/// 行级 LCS 的 DP 表上限（行数乘积）。超过后回退为"中间区域整体替换"，避免 reload 大文件时 diff 计算卡死。
const MAX_LCS_CELLS: usize = 4_000_000;

/// 把 `old` 变为 `new` 的净变化 patch；
/// 匹配区域不产生段。
///
/// 行级公共前后缀先行裁剪缩小 LCS 规模；
/// 每个替换段再做字符级前后缀细化，使行内小修改也能精确映射段内坐标。
pub(crate) fn diff_patch(old: &str, new: &str) -> TextPatch {
    let old_lines = split_lines(old);
    let new_lines = split_lines(new);
    let old_offsets = line_offsets(&old_lines);
    let new_offsets = line_offsets(&new_lines);

    // 行级公共前后缀：逐行相等即为匹配，缩小中间区域的 LCS 规模。
    let prefix = old_lines
        .iter()
        .zip(new_lines.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old_lines
        .iter()
        .rev()
        .zip(new_lines.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(old_lines.len() - prefix)
        .min(new_lines.len() - prefix);

    let old_mid = &old_lines[prefix..old_lines.len() - suffix];
    let new_mid = &new_lines[prefix..new_lines.len() - suffix];
    let matches = if old_mid.len() * new_mid.len() > MAX_LCS_CELLS {
        Vec::new()
    } else {
        lcs_matches(old_mid, new_mid)
    };

    // 前后缀行是必然匹配，与中间 LCS 的匹配对（下标平移回全文）合并。
    let mut pairs = Vec::with_capacity(prefix + suffix + matches.len());
    pairs.extend((0..prefix).map(|i| (i, i)));
    pairs.extend(matches.into_iter().map(|(i, j)| (i + prefix, j + prefix)));
    pairs.extend((0..suffix).map(|i| (old_lines.len() - suffix + i, new_lines.len() - suffix + i)));

    build_patch(old, new, &old_offsets, &new_offsets, &pairs)
}

/// 按 `\n` 切分文本为行（保留换行符；末行可能没有换行符）。
fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// 每行起始字节偏移；末尾多一个文本总长哨兵，供末段计算。
fn line_offsets(lines: &[&str]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(lines.len() + 1);
    let mut offset = 0;
    for line in lines {
        offsets.push(offset);
        offset += line.len();
    }
    offsets.push(offset);
    offsets
}

/// 中间区域的行级最长公共子序列匹配对（严格递增）。
///
/// 回溯需要任意 (i, j) 的 DP 值（滚动数组无法支撑），因此保留完整表；
/// 这里用一维行主序数组替代 `Vec<Vec>`：单块分配、按列反向迭代时两行访问都连续。
fn lcs_matches(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let (n, m) = (old.len(), new.len());
    // dp[i][j] = old[i..] 与 new[j..] 的 LCS 长度。
    let width = m + 1;
    let mut dp = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * width + j] = if old[i] == new[j] {
                dp[(i + 1) * width + j + 1] + 1
            } else {
                dp[(i + 1) * width + j].max(dp[i * width + j + 1])
            };
        }
    }
    // 回溯收集匹配对。
    let mut matches = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old[i] == new[j] {
            matches.push((i, j));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * width + j] >= dp[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    matches
}

/// 从匹配行对序列构建 patch：相邻匹配之间是替换段。
fn build_patch(
    old: &str,
    new: &str,
    old_offsets: &[usize],
    new_offsets: &[usize],
    pairs: &[(usize, usize)],
) -> TextPatch {
    let mut edits = Vec::new();
    let mut prev_old = 0usize;
    let mut prev_new = 0usize;
    for (old_line, new_line) in pairs {
        push_refined_edit(
            old,
            new,
            old_offsets[prev_old],
            old_offsets[*old_line],
            new_offsets[prev_new],
            new_offsets[*new_line],
            &mut edits,
        );
        prev_old = old_line + 1;
        prev_new = new_line + 1;
    }
    push_refined_edit(
        old,
        new,
        old_offsets[prev_old],
        old.len(),
        new_offsets[prev_new],
        new.len(),
        &mut edits,
    );
    TextPatch::from_edits(edits)
}

/// 行粒度替换段内部再做字符级公共前后缀，把段细化为最小的替换区。
///
/// 字节级比较天然落在 UTF-8 字符边界，不会切分多字节字符。
fn push_refined_edit(
    old: &str,
    new: &str,
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
    edits: &mut Vec<PatchEdit>,
) {
    let old_slice = &old[old_start..old_end];
    let new_slice = &new[new_start..new_end];
    let prefix = old_slice
        .bytes()
        .zip(new_slice.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old_slice
        .bytes()
        .rev()
        .zip(new_slice.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(old_slice.len() - prefix)
        .min(new_slice.len() - prefix);
    let inner_old_start = old_start + prefix;
    let inner_new_start = new_start + prefix;
    let inner_old_end = old_end - suffix;
    let inner_new_end = new_end - suffix;

    // 段完全匹配（前后缀相接）时无编辑。
    if inner_old_start != inner_old_end || inner_new_start != inner_new_end {
        edits.push(PatchEdit::new(
            inner_old_start..inner_old_end,
            inner_new_start..inner_new_end,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ByteOffset;
    use crate::PositionMap;

    fn map_offset(patch: &TextPatch, offset: usize) -> usize {
        let position_map = PositionMap::from_text_patch(patch);
        position_map
            .map_old_position(ByteOffset::new(offset))
            .value()
            .get()
    }

    #[test]
    fn identical_texts_produce_empty_patch() {
        assert!(diff_patch("a\nb\nc", "a\nb\nc").is_empty());
        assert!(diff_patch("", "").is_empty());
    }

    #[test]
    fn line_insertion_maps_surrounding_offsets() {
        let patch = diff_patch("a\nb\nc", "a\nx\nb\nc");
        // "a\n" 匹配；光标在 "b" 行内 offset 2 处应平移到插入行之后。
        assert_eq!(map_offset(&patch, 2), 4);
        assert_eq!(map_offset(&patch, 6), 8);
    }

    #[test]
    fn line_deletion_maps_offsets_to_delete_start() {
        let patch = diff_patch("a\nx\nb\nc", "a\nb\nc");
        // 被删行 "x\n"（offset 2..4）内的坐标塌缩到删除起点。
        assert_eq!(map_offset(&patch, 3), 2);
        assert_eq!(map_offset(&patch, 4), 2);
        assert_eq!(map_offset(&patch, 6), 4);
    }

    #[test]
    fn inline_edit_refines_to_smallest_replace_span() {
        let patch = diff_patch("alpha\nbravo\ncharlie", "alpha\nbrxavo\ncharlie");
        // "br" 与 "avo\n" 字符级匹配，只有中间的 "x" 是替换段：
        // 光标在 "br" 后（offset 8）映射到插入 "x" 之后（offset 9）。
        assert_eq!(map_offset(&patch, 8), 9);
    }

    #[test]
    fn complete_rewrite_falls_back_to_whole_replace() {
        let patch = diff_patch("alpha\nbravo", "xyz\nqwerty");
        assert_eq!(patch.edits().len(), 1);
        let edit = &patch.edits()[0];
        assert_eq!(edit.old_range().start().get(), 0);
        assert_eq!(edit.old_range().end().get(), "alpha\nbravo".len());
        assert_eq!(edit.new_range().start().get(), 0);
        assert_eq!(edit.new_range().end().get(), "xyz\nqwerty".len());
        // 被替换内容内的坐标塌缩到替换段起点。
        assert_eq!(map_offset(&patch, 5), 0);
    }

    #[test]
    fn empty_line_changes_map_correctly() {
        let patch = diff_patch("a\n\nb", "a\n\n\nb");
        // 中间插入一个空行：第二个空行前的 "a\n\n" 匹配，"b" 后移一行。
        assert_eq!(map_offset(&patch, 4), 5);
    }
}
