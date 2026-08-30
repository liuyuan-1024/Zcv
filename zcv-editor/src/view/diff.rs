//! diff 显示语义：hunks（逻辑行）→ 行级渲染数据。
//!
//! 与像素渲染无关的"逻辑行→显示行"映射集中在这里；
//! 渲染端只消费计算出的 `HunkRendering` 做布局与绘制。

use std::ops::Range;

use zcv_git::{DiffHunk, DiffHunkKind};
use zcv_text::Line;

use crate::display_map::DisplaySnapshot;

/// hunks 的单遍渲染数据：行标记 / 竖条 / 点击区域共用同一份行区间计算。
pub(crate) struct HunkRendering {
    pub(crate) diff_rows: Vec<(Range<usize>, DiffHunkKind)>,
    pub(crate) strips: Vec<(Range<usize>, DiffHunkKind)>,
    pub(crate) hit_regions: Vec<(Range<usize>, Range<usize>, DiffHunkKind)>,
    /// hunk 操作栏的锚定显示范围；控件取范围起点作为右上角所在行。
    pub(crate) controls: Vec<(Range<usize>, DiffHunk)>,
    /// 需要整行差异背景的显示行区间；新增行始终包含，修改/删除只在展开时包含。
    pub(crate) expanded_rows: Vec<Range<usize>>,
}

/// hunks（逻辑行）→ 行级渲染数据，单遍遍历产出四份视图：
///
/// - `diff_rows`：行标记（gutter 指示，wrap 下行映射出的全部显示行都覆盖）
/// - `strips`：竖条范围与状态色（竖条颜色不随展开变化）
/// - `hit_regions`：可点击色带区域（显示行范围 + 点击目标 old_range + 类型）
/// - `expanded_rows`：整行差异背景数据源（新增行始终着色，修改/删除按展开态着色）
///
/// 覆盖终点取 hunk 之后第一行的行首显示行（end 行首显示行 − 1 即 hunk 最后一个显示行，左闭右开区间 [start, end) 恰好盖住全部 wrap 片段）；
/// hunk 到达文件末尾时以显示快照行数为终点。
/// 纯删除 hunk（空范围）：折叠时行内不做标记（gutter 红色三角提示），展开后标记物化的旧侧行；
/// 修改 hunk 展开后：旧侧行按删除色、修改行按新增色（base 旧行红、新行绿）。
/// 映射失败（越界等）跳过该 hunk。
pub(crate) fn hunk_rendering(
    snapshot: &DisplaySnapshot,
    hunks: &[DiffHunk],
    expanded_deleted: &[Range<usize>],
    expanded_modified: &[Range<usize>],
    old_display_ranges: &[Option<Range<usize>>],
) -> HunkRendering {
    let mut diff_rows = Vec::new();
    let mut strips = Vec::new();
    let mut hit_regions = Vec::new();
    let mut controls = Vec::new();
    let mut expanded_rows = Vec::new();
    for (index, hunk) in hunks.iter().enumerate() {
        let old_rows = old_display_ranges
            .get(index)
            .and_then(|range| range.as_ref())
            .and_then(|range| logical_rows(snapshot, range));
        let new_rows = logical_rows(snapshot, &hunk.range);
        match hunk.kind {
            DiffHunkKind::Added => {
                if let Some(rows) = new_rows {
                    diff_rows.push((rows.clone(), DiffHunkKind::Added));
                    strips.push((rows.clone(), DiffHunkKind::Added));
                    expanded_rows.push(rows.clone());
                    controls.push((rows, hunk.clone()));
                }
            }
            DiffHunkKind::Deleted => {
                let expanded = expanded_deleted.contains(&hunk.old_range);
                if expanded && let Some(rows) = old_rows {
                    diff_rows.push((rows.clone(), DiffHunkKind::Deleted));
                    strips.push((rows.clone(), DiffHunkKind::Deleted));
                    expanded_rows.push(rows.clone());
                    hit_regions.push((rows.clone(), hunk.old_range.clone(), DiffHunkKind::Deleted));
                    controls.push((rows, hunk.clone()));
                } else if let Some(rows) =
                    old_rows.or_else(|| logical_anchor_rows(snapshot, hunk.range.start))
                {
                    hit_regions.push((rows.clone(), hunk.old_range.clone(), DiffHunkKind::Deleted));
                    controls.push((rows, hunk.clone()));
                }
            }
            DiffHunkKind::Modified => {
                let expanded = expanded_modified.contains(&hunk.old_range);
                if expanded && let (Some(old_rows), Some(new_rows)) = (&old_rows, &new_rows) {
                    diff_rows.push((old_rows.clone(), DiffHunkKind::Deleted));
                    diff_rows.push((new_rows.clone(), DiffHunkKind::Added));
                    expanded_rows.push(old_rows.clone());
                    expanded_rows.push(new_rows.clone());
                    let rows = old_rows.start.min(new_rows.start)..old_rows.end.max(new_rows.end);
                    strips.push((rows.clone(), DiffHunkKind::Modified));
                    hit_regions.push((
                        rows.clone(),
                        hunk.old_range.clone(),
                        DiffHunkKind::Modified,
                    ));
                    controls.push((rows, hunk.clone()));
                } else if let Some(rows) = new_rows {
                    diff_rows.push((rows.clone(), DiffHunkKind::Modified));
                    strips.push((rows.clone(), DiffHunkKind::Modified));
                    hit_regions.push((
                        rows.clone(),
                        hunk.old_range.clone(),
                        DiffHunkKind::Modified,
                    ));
                    controls.push((rows, hunk.clone()));
                }
            }
        }
    }
    HunkRendering {
        diff_rows,
        strips,
        hit_regions,
        controls,
        expanded_rows,
    }
}

fn logical_rows(snapshot: &DisplaySnapshot, range: &Range<usize>) -> Option<Range<usize>> {
    if range.is_empty() {
        return None;
    }
    let start = snapshot.line_to_display_row(Line::new(range.start))?.get();
    let end = snapshot
        .line_to_display_row(Line::new(range.end))
        .map_or_else(|| snapshot.line_count(), |row| row.get());
    Some(start..end.max(start + 1))
}

fn logical_anchor_rows(snapshot: &DisplaySnapshot, line: usize) -> Option<Range<usize>> {
    let row = snapshot.line_to_display_row(Line::new(line))?.get();
    Some(row..row + 1)
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
    use crate::display_map::DisplayMap;
    use zcv_text::{Buffer, BufferConfig};

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

    #[test]
    fn every_diff_hunk_exposes_a_control_anchor() {
        let buffer = Buffer::scratch(
            "line0\nline1\nline2\nline3\nline4\n".into(),
            BufferConfig::default(),
        )
        .expect("应创建测试 Buffer");
        let snapshot = DisplayMap::new(buffer.snapshot()).snapshot();
        let hunks = vec![
            DiffHunk {
                range: 0..1,
                old_range: 0..0,
                kind: DiffHunkKind::Added,
            },
            DiffHunk {
                range: 2..3,
                old_range: 2..3,
                kind: DiffHunkKind::Modified,
            },
            DiffHunk {
                range: 4..4,
                old_range: 4..5,
                kind: DiffHunkKind::Deleted,
            },
        ];

        let rendered = hunk_rendering(&snapshot, &hunks, &[], &[], &[None, None, None]);
        assert_eq!(
            rendered
                .controls
                .iter()
                .map(|(rows, hunk)| (rows.start, hunk.kind))
                .collect::<Vec<_>>(),
            vec![
                (0, DiffHunkKind::Added),
                (2, DiffHunkKind::Modified),
                (4, DiffHunkKind::Deleted),
            ]
        );
        assert_eq!(rendered.expanded_rows, vec![0..1]);
    }

    #[test]
    fn materialized_modified_hunk_uses_real_old_and_new_document_rows() {
        let buffer = Buffer::scratch("context\nold\nnew\nafter\n".into(), BufferConfig::default())
            .expect("应创建测试 Buffer");
        let snapshot = DisplayMap::new(buffer.snapshot()).snapshot();
        let hunk = DiffHunk {
            range: 2..3,
            old_range: 10..11,
            kind: DiffHunkKind::Modified,
        };
        let old_ranges = vec![Some(1..2)];

        let rendered = hunk_rendering(
            &snapshot,
            std::slice::from_ref(&hunk),
            &[],
            std::slice::from_ref(&(10..11)),
            &old_ranges,
        );

        assert_eq!(
            rendered.diff_rows,
            vec![(1..2, DiffHunkKind::Deleted), (2..3, DiffHunkKind::Added)]
        );
        assert_eq!(rendered.strips, vec![(1..3, DiffHunkKind::Modified)]);
        assert_eq!(rendered.controls, vec![(1..3, hunk)]);
        assert_eq!(
            rendered.hit_regions,
            vec![(1..3, 10..11, DiffHunkKind::Modified)],
            "物化旧侧与普通编辑器共用 gutter 折叠入口"
        );
    }
}
