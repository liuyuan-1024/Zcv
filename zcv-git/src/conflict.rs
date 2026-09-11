//! Git 冲突标记的纯文本解析。
//!
//! 这里只识别 Git 生成的 `<<<<<<<`、`=======`、`>>>>>>>` 标记，返回原始文本中的字节范围；
//! 不持有工作区、Buffer 或编辑器状态。
//!
//! 所有范围都是 UTF-8 字节偏移，调用方必须基于同一份文本快照消费这些范围。

use std::ops::Range;

/// 冲突解决时保留的内容来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    /// 保留当前分支（ours）的内容。
    Ours,
    /// 保留传入分支（theirs）的内容。
    Theirs,
    /// 按当前冲突顺序保留双方内容。
    Both,
}

/// 单个冲突块在原始文本中的字节范围。
///
/// `outer` 包含全部 Git 冲突标记；
/// `ours` 和 `theirs` 只覆盖双方正文，不包含标记行。
/// 所有范围均来自同一份文本快照。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictRegion {
    /// 从当前侧起始标记到传入侧结束标记的完整范围。
    pub outer: Range<usize>,
    /// 当前侧正文范围。
    pub ours: Range<usize>,
    /// 传入侧正文范围。
    pub theirs: Range<usize>,
    /// 当前侧起始标记中的分支名称。
    pub ours_branch: String,
    /// 传入侧结束标记中的分支名称。
    pub theirs_branch: String,
}

/// 解析文本中的 Git 冲突块。
///
/// 只返回同时包含当前侧、分隔线和传入侧结束标记的完整冲突；
/// 未闭合或不完整的标记不会生成区域。
/// 返回范围使用 UTF-8 字节偏移。
pub fn parse_conflict_regions(text: &str) -> Vec<ConflictRegion> {
    let mut conflicts = Vec::new();
    let mut current: Option<ConflictBuilder> = None;
    let mut offset = 0;

    for line in text.split_inclusive('\n') {
        let line_end = offset + line.len();
        let marker = line.trim_end_matches(['\n', '\r']);
        if let Some(branch) = marker.strip_prefix("<<<<<<< ") {
            current = Some(ConflictBuilder {
                outer_start: offset,
                ours_start: line_end,
                ours_end: None,
                theirs_start: None,
                ours_branch: branch.trim().to_owned(),
            });
        } else if let Some(builder) = current.as_mut() {
            if marker.starts_with("||||||| ") && builder.ours_end.is_none() {
                builder.ours_end = Some(offset);
            } else if marker == "=======" {
                if builder.ours_end.is_none() {
                    builder.ours_end = Some(offset);
                }
                builder.theirs_start = Some(line_end);
            } else if let Some(branch) = marker.strip_prefix(">>>>>>> ") {
                if let (Some(ours_end), Some(theirs_start)) =
                    (builder.ours_end, builder.theirs_start)
                {
                    conflicts.push(ConflictRegion {
                        outer: builder.outer_start..line_end,
                        ours: builder.ours_start..ours_end,
                        theirs: theirs_start..offset,
                        ours_branch: builder.ours_branch.clone(),
                        theirs_branch: branch.trim().to_owned(),
                    });
                }
                current = None;
            }
        }
        offset = line_end;
    }

    conflicts
}

/// 根据选择移除一个冲突块的 Git 标记，并返回解决后的完整文本。
pub fn resolve_conflict(text: &str, region: &ConflictRegion, choice: ConflictChoice) -> String {
    let replacement = match choice {
        ConflictChoice::Ours => &text[region.ours.clone()],
        ConflictChoice::Theirs => &text[region.theirs.clone()],
        ConflictChoice::Both => {
            let mut both = String::with_capacity(region.ours.len() + region.theirs.len());
            both.push_str(&text[region.ours.clone()]);
            both.push_str(&text[region.theirs.clone()]);
            return replace_region(text, &region.outer, &both);
        }
    };
    replace_region(text, &region.outer, replacement)
}

fn replace_region(text: &str, range: &Range<usize>, replacement: &str) -> String {
    let mut resolved = String::with_capacity(text.len() - range.len() + replacement.len());
    resolved.push_str(&text[..range.start]);
    resolved.push_str(replacement);
    resolved.push_str(&text[range.end..]);
    resolved
}

/// 解析尚未遇到结束标记的冲突块。
struct ConflictBuilder {
    /// 冲突起始标记的字节位置。
    outer_start: usize,
    /// 当前侧正文的起始位置。
    ours_start: usize,
    /// 当前侧正文的结束位置。
    ours_end: Option<usize>,
    /// 传入侧正文的起始位置。
    theirs_start: Option<usize>,
    /// 当前侧起始标记中的分支名称。
    ours_branch: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "before\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\nafter\n";

    #[test]
    fn resolves_each_side_without_leaving_markers() {
        let conflict = &parse_conflict_regions(TEXT)[0];
        assert_eq!(
            resolve_conflict(TEXT, conflict, ConflictChoice::Ours),
            "before\nours\nafter\n"
        );
        assert_eq!(
            resolve_conflict(TEXT, conflict, ConflictChoice::Theirs),
            "before\ntheirs\nafter\n"
        );
        assert_eq!(
            resolve_conflict(TEXT, conflict, ConflictChoice::Both),
            "before\nours\ntheirs\nafter\n"
        );
    }
}
