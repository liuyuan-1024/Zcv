//! diff 显示语义：hunks（逻辑行）→ 行级渲染数据。
//!
//! 与像素渲染无关的"逻辑行→显示行"映射集中在这里（对齐 Zed 把 diff 渲染数据计算放在独立单元的做法）；
//! 渲染端只消费计算出的 `HunkRendering` 做布局与绘制。

use std::ops::Range;

use zcv_engine::Line;
use zcv_git::{DiffHunk, DiffHunkKind};

use crate::display_map::{DisplayRow, DisplaySnapshot, StreamLineSource, WrapViewportRowKind};

/// hunks 的单遍渲染数据：行标记 / 竖条 / 点击区域共用同一份行区间计算。
pub(crate) struct HunkRendering {
    pub(crate) diff_rows: Vec<(Range<usize>, DiffHunkKind)>,
    pub(crate) strips: Vec<(Range<usize>, DiffHunkKind)>,
    pub(crate) hit_regions: Vec<(Range<usize>, Range<usize>, DiffHunkKind)>,
}

/// hunks（逻辑行）→ 行级渲染数据，单遍遍历产出三份视图：
///
/// - `diff_rows`：行标记（gutter 指示与内容背景，wrap 下行映射出的全部显示行都覆盖）
/// - `strips`：竖条范围与状态色（竖条颜色不随展开变化）
/// - `hit_regions`：可点击色带区域（显示行范围 + 点击目标 old_range + 类型）
///
/// 覆盖终点取 hunk 之后第一行的行首显示行（对齐 Zed：end 行首显示行 − 1 即 hunk 最后一个显示行，左闭右开区间 [start, end) 恰好盖住全部 wrap 片段）；
/// hunk 到达文件末尾时以显示快照行数为终点。
/// 纯删除 hunk（空范围）：折叠时行内不做标记（gutter 红色三角提示），展开后标记移到被删除的合成行上；
/// 修改 hunk 展开后：旧行（合成行）按删除色、修改行按新增色（对齐 Zed：base 旧行红、新行绿）。
/// 映射失败（越界等）跳过该 hunk。
pub(crate) fn hunk_rendering(
    snapshot: &DisplaySnapshot,
    hunks: &[DiffHunk],
    expanded_deleted: &[Range<usize>],
    expanded_modified: &[Range<usize>],
) -> HunkRendering {
    let mut diff_rows = Vec::new();
    let mut strips = Vec::new();
    let mut hit_regions = Vec::new();
    for hunk in hunks {
        match hunk.kind {
            DiffHunkKind::Added => {
                let Some(start) = snapshot.line_to_display_row(Line::new(hunk.range.start)) else {
                    continue;
                };
                let start = start.get();
                let end = match snapshot.line_to_display_row(Line::new(hunk.range.end)) {
                    Some(row) => row.get(),
                    None => snapshot.line_count(),
                };
                let rows = start..end.max(start + 1);
                diff_rows.push((rows.clone(), DiffHunkKind::Added));
                strips.push((rows, DiffHunkKind::Added));
            }
            DiffHunkKind::Modified => {
                let expanded = expanded_modified.contains(&hunk.old_range);
                if expanded {
                    // 展开：旧行（合成行，删除色）+ 修改行（新增色）。
                    let old_rows = modified_hunk_rows(snapshot, hunk, true);
                    let new_rows = modified_hunk_rows(snapshot, hunk, false);
                    if let Some(old_rows) = &old_rows {
                        diff_rows.push((old_rows.clone(), DiffHunkKind::Deleted));
                    }
                    if let Some(new_rows) = &new_rows {
                        diff_rows.push((new_rows.clone(), DiffHunkKind::Added));
                    }
                    if let (Some(old_rows), Some(new_rows)) = (&old_rows, &new_rows) {
                        // 竖条与点击区域覆盖整个 hunk（旧行 + 修改行）。
                        let rows = old_rows.start..new_rows.end;
                        strips.push((rows.clone(), DiffHunkKind::Modified));
                        hit_regions.push((rows, hunk.old_range.clone(), DiffHunkKind::Modified));
                    }
                } else if let Some(hunk_rows) = modified_hunk_rows(snapshot, hunk, false) {
                    diff_rows.push((hunk_rows.clone(), DiffHunkKind::Modified));
                    strips.push((hunk_rows.clone(), DiffHunkKind::Modified));
                    hit_regions.push((hunk_rows, hunk.old_range.clone(), DiffHunkKind::Modified));
                }
            }
            DiffHunkKind::Deleted => {
                let expanded = expanded_deleted.contains(&hunk.old_range);
                // 折叠态点击区域锚定在删除点行（gutter 三角提示）；展开态覆盖合成行。
                let rows = deleted_hunk_rows(snapshot, hunk, expanded);
                if expanded && let Some(rows) = &rows {
                    diff_rows.push((rows.clone(), DiffHunkKind::Deleted));
                    strips.push((rows.clone(), DiffHunkKind::Deleted));
                }
                if let Some(rows) = rows {
                    hit_regions.push((rows, hunk.old_range.clone(), DiffHunkKind::Deleted));
                }
            }
        }
    }
    HunkRendering {
        diff_rows,
        strips,
        hit_regions,
    }
}

/// 纯删除 hunk 的显示行范围：未展开 = 锚定行（删除点所在显示行，色带顶部指向分界线）；
/// 展开 = 锚定行之后的连续合成行（被删除行显示在原位置，左闭右开）。
fn deleted_hunk_rows(
    snapshot: &DisplaySnapshot,
    hunk: &DiffHunk,
    expanded: bool,
) -> Option<Range<usize>> {
    let anchor = snapshot
        .line_to_display_row(Line::new(hunk.range.start))?
        .get();
    if !expanded {
        return Some(anchor..anchor + 1);
    }
    // 被删合成行插在锚定行（range.start）之后：从锚定行后第一个合成行数到连续合成行结束。
    let line_count = snapshot.line_count();
    let mut start = anchor + 1;
    while start < line_count && !display_row_is_inserted(snapshot, start) {
        start += 1;
    }
    let mut end = start;
    while end < line_count && display_row_is_inserted(snapshot, end) {
        end += 1;
    }
    (start < end).then_some(start..end)
}

/// 显示行是否为合成行（外部文本；wrap 片段同样计入）。
fn display_row_is_inserted(snapshot: &DisplaySnapshot, row: usize) -> bool {
    snapshot
        .slice_viewport(DisplayRow::new(row), 1)
        .is_ok_and(|viewport| {
            viewport.rows().first().is_some_and(|row| {
                matches!(
                    row.kind(),
                    WrapViewportRowKind::Text {
                        source: StreamLineSource::Inserted { .. },
                        ..
                    }
                )
            })
        })
}

/// 可点击的删除块色带区域（显示行范围 + old_range；折叠/展开两态都覆盖，供 gutter 点击切换）。
/// 修改 hunk 的显示行范围：未展开 = 修改行本身；展开 = 修改行上方的连续合成行（旧行）。
fn modified_hunk_rows(
    snapshot: &DisplaySnapshot,
    hunk: &DiffHunk,
    expanded: bool,
) -> Option<Range<usize>> {
    let anchor = snapshot
        .line_to_display_row(Line::new(hunk.range.start))?
        .get();
    if !expanded {
        let end = match snapshot.line_to_display_row(Line::new(hunk.range.end)) {
            Some(row) => row.get(),
            None => snapshot.line_count(),
        };
        return Some(anchor..end.max(anchor + 1));
    }
    // 合成行（HEAD 旧行）插在修改行上方：从修改行首显示行往前数连续合成行。
    let mut start = anchor;
    while start > 0 && display_row_is_inserted(snapshot, start - 1) {
        start -= 1;
    }
    (start < anchor).then_some(start..anchor)
}

/// 查询显示行所属的 diff 类型（gutter 与内容背景共用；线性扫描，hunks 数量级小）。
pub(crate) fn diff_kind_for_row(
    diff_rows: &[(Range<usize>, DiffHunkKind)],
    row: usize,
) -> Option<DiffHunkKind> {
    diff_rows
        .iter()
        .find(|(range, _)| range.contains(&row))
        .map(|(_, kind)| *kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_kind_for_row_matches_display_row_ranges() {
        // 输入是 diff_hunk_rows 的输出：Deleted 已从空区间展开为锚定行的单行区间。
        let diff_rows = vec![
            (2..5, DiffHunkKind::Modified),
            (7..8, DiffHunkKind::Deleted),
        ];

        assert_eq!(diff_kind_for_row(&diff_rows, 1), None);
        assert_eq!(
            diff_kind_for_row(&diff_rows, 2),
            Some(DiffHunkKind::Modified)
        );
        assert_eq!(
            diff_kind_for_row(&diff_rows, 4),
            Some(DiffHunkKind::Modified)
        );
        assert_eq!(diff_kind_for_row(&diff_rows, 5), None);
        assert_eq!(
            diff_kind_for_row(&diff_rows, 7),
            Some(DiffHunkKind::Deleted)
        );
        assert_eq!(diff_kind_for_row(&diff_rows, 8), None);
        assert_eq!(diff_kind_for_row(&[], 0), None);
    }
}
