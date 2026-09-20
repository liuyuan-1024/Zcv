use super::*;
use crate::display_map::DisplayMap;
use gpui::{AppContext, Empty, Entity, TestAppContext, px};
use zcv_multi_buffer::{MultiBufferSnapshot, ResolvedDiffHunk};
use zcv_text::{Buffer, BufferConfig};

/// 由绝对坐标切片构造按段解析输入，供渲染单元测试调用。
fn resolved_hunks(
    hunks: &[DisplayHunk],
    expanded: &[bool],
    old_ranges: &[Option<Range<usize>>],
    word_diffs: &[WordDiffs],
) -> Vec<ResolvedDiffHunk> {
    hunks
        .iter()
        .enumerate()
        .map(|(index, hunk)| ResolvedDiffHunk {
            hunk: hunk.clone(),
            old_range: old_ranges.get(index).cloned().flatten(),
            expanded: expanded.get(index).copied().unwrap_or(false),
            word_diffs: word_diffs.get(index).cloned().unwrap_or_default(),
        })
        .collect()
}

fn new_display_map(
    cx: &mut impl AppContext,
    snapshot: impl Into<MultiBufferSnapshot>,
) -> Entity<DisplayMap> {
    cx.new(|cx| DisplayMap::new(snapshot, cx))
}

fn project_display_snapshot(
    cx: &mut impl AppContext,
    snapshot: impl Into<MultiBufferSnapshot>,
) -> DisplaySnapshot {
    let map = new_display_map(cx, snapshot);
    cx.update_entity(&map, |map, cx| map.snapshot(cx))
}

impl SearchDecorationSnapshot {
    pub(crate) fn for_test(
        display: &DisplaySnapshot,
        ranges: &[MultiBufferRange],
        active_index: usize,
    ) -> Self {
        let projected_rows = ranges
            .iter()
            .flat_map(|range| display.project_text_range(*range).unwrap_or_default())
            .map(projected_row_range)
            .collect::<Arc<[_]>>();
        let projected_rows_lock = OnceLock::new();
        let _ = projected_rows_lock.set(projected_rows);
        Self {
            ranges: Arc::from(ranges),
            active_index,
            projected_rows: projected_rows_lock,
        }
    }

    pub(crate) fn projected_rows_for_test(&self) -> &[Range<usize>] {
        self.projected_rows.get().map_or(&[], |rows| rows.as_ref())
    }
}

#[gpui::test]
fn folded_deleted_hunk_anchor_covers_all_wrapped_subrows(cx: &mut TestAppContext) {
    // 删除点逻辑行软换行拆成多个子行时，折叠的纯删除块锚点必须覆盖全部子行：
    // 三角标记落在删除点行尾（最后子行行尾 = 与下一行的边界），点击区域整行可点，而不是只落在第一个子行之间。
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let text_system = window.text_system().clone();
            let font = window.text_style().font();
            let font_size = window.text_style().font_size.to_pixels(window.rem_size());
            let lines = (0..24)
                .map(|i| {
                    if i == 20 {
                        "x".repeat(120)
                    } else {
                        format!("line {i}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let buffer =
                Buffer::from_text(lines, BufferConfig::default()).expect("应创建测试 Buffer");
            let map = new_display_map(cx, buffer.snapshot());
            assert!(
                cx.update_entity(&map, |map, cx| map.set_wrap_width(
                    Some(px(100.)),
                    font.clone(),
                    font_size,
                    &text_system,
                    cx
                )),
                "第 20 行应产生软换行"
            );
            let snapshot = cx.update_entity(&map, |map, cx| map.snapshot(cx));
            let start_row = snapshot
                .line_to_display_row(Line::new(20))
                .expect("第 20 行应可映射");
            let end_row = snapshot
                .line_to_display_row(Line::new(21))
                .expect("第 21 行应可映射");
            assert!(
                end_row.get() > start_row.get() + 1,
                "软换行行应拆成多个子行"
            );

            let hunk = DisplayHunk {
                range: 20..20,
                old_range: 20..21,
                kind: DiffHunkKind::Deleted,
                staging: DiffHunkStaging::NoStaging,
            };
            let rendered = hunk_rendering(
                &snapshot,
                resolved_hunks(std::slice::from_ref(&hunk), &[false], &[None], &[]).into_iter(),
            );
            assert_eq!(
                rendered.hit_regions,
                vec![(start_row.get()..end_row.get(), 0, DiffHunkKind::Deleted)],
                "折叠删除块锚点应覆盖软换行的全部子行（三角落在删除点行尾）"
            );
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn folded_deleted_hunk_before_wrapped_row_anchors_to_wrapped_first_subrow(cx: &mut TestAppContext) {
    // 删除点行（第 15 行）无软换行、紧跟在它后面的第 16 行软换行：
    // 三角落在删除点行行尾 = 软换行第一子行行首（被删行在软换行之前）。
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let text_system = window.text_system().clone();
            let font = window.text_style().font();
            let font_size = window.text_style().font_size.to_pixels(window.rem_size());
            let lines = (0..24)
                .map(|i| {
                    if i == 16 {
                        "x".repeat(120)
                    } else {
                        format!("line {i}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let buffer =
                Buffer::from_text(lines, BufferConfig::default()).expect("应创建测试 Buffer");
            let map = new_display_map(cx, buffer.snapshot());
            assert!(
                cx.update_entity(&map, |map, cx| map.set_wrap_width(
                    Some(px(100.)),
                    font.clone(),
                    font_size,
                    &text_system,
                    cx
                )),
                "第 16 行应产生软换行"
            );
            let snapshot = cx.update_entity(&map, |map, cx| map.snapshot(cx));
            let del_start = snapshot
                .line_to_display_row(Line::new(15))
                .expect("删除点行应可映射")
                .get();
            let wrapped_first = snapshot
                .line_to_display_row(Line::new(16))
                .expect("第 16 行应可映射")
                .get();
            let wrapped_next = snapshot
                .line_to_display_row(Line::new(17))
                .expect("第 17 行应可映射")
                .get();
            assert!(wrapped_next > wrapped_first + 1, "第 16 行应拆成多个子行");
            assert_eq!(wrapped_first, del_start + 1, "删除点行应为单显示行");

            let hunk = DisplayHunk {
                range: 15..15,
                old_range: 15..16,
                kind: DiffHunkKind::Deleted,
                staging: DiffHunkStaging::NoStaging,
            };
            let rendered = hunk_rendering(
                &snapshot,
                resolved_hunks(std::slice::from_ref(&hunk), &[false], &[None], &[]).into_iter(),
            );
            // 删除点行是单行 [del_start, del_start+1)，三角在该行行尾 = 软换行第一子行行首。
            assert_eq!(
                rendered.hit_regions,
                vec![(del_start..del_start + 1, 0, DiffHunkKind::Deleted)],
                "删除点在软换行之前时锚点应落在软换行第一子行行首"
            );
        })
        .expect("测试窗口应保持可用");
}

#[test]
fn diff_kind_for_row_matches_display_row_ranges() {
    // 输入是 diff_hunk_rows 的输出：Deleted 已从空区间展开为锚定行的单行区间。
    let diff_rows = vec![
        (2..5, DiffHunkKind::Modified, DiffHunkStaging::NoStaging),
        (7..8, DiffHunkKind::Deleted, DiffHunkStaging::NoStaging),
    ];

    assert_eq!(diff_row_for_row(&diff_rows, 1), None);
    assert_eq!(
        diff_row_for_row(&diff_rows, 2),
        Some((DiffHunkKind::Modified, DiffHunkStaging::NoStaging))
    );
    assert_eq!(
        diff_row_for_row(&diff_rows, 4),
        Some((DiffHunkKind::Modified, DiffHunkStaging::NoStaging))
    );
    assert_eq!(diff_row_for_row(&diff_rows, 5), None);
    assert_eq!(
        diff_row_for_row(&diff_rows, 7),
        Some((DiffHunkKind::Deleted, DiffHunkStaging::NoStaging))
    );
    assert_eq!(diff_row_for_row(&diff_rows, 8), None);
    assert_eq!(diff_row_for_row(&[], 0), None);
}

#[gpui::test]
fn every_diff_hunk_exposes_a_control_anchor(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text(
        "line0\nline1\nline2\nline3\nline4\n".into(),
        BufferConfig::default(),
    )
    .expect("应创建测试 Buffer");
    let snapshot = project_display_snapshot(cx, buffer.snapshot());
    let hunks = vec![
        DisplayHunk {
            range: 0..1,
            old_range: 0..0,
            kind: DiffHunkKind::Added,
            staging: DiffHunkStaging::NoStaging,
        },
        DisplayHunk {
            range: 2..3,
            old_range: 2..3,
            kind: DiffHunkKind::Modified,
            staging: DiffHunkStaging::NoStaging,
        },
        DisplayHunk {
            range: 4..4,
            old_range: 4..5,
            kind: DiffHunkKind::Deleted,
            staging: DiffHunkStaging::NoStaging,
        },
    ];

    let rendered = hunk_rendering(
        &snapshot,
        resolved_hunks(&hunks, &[false, false, false], &[None, None, None], &[]).into_iter(),
    );
    assert_eq!(
        rendered
            .controls
            .iter()
            .map(|(rows, hunk)| {
                let HunkControlTarget::Diff(hunk) = hunk else {
                    unreachable!("普通 diff 渲染不应产生自定义 hunk")
                };
                (rows.start, hunk.kind)
            })
            .collect::<Vec<_>>(),
        vec![
            (0, DiffHunkKind::Added),
            (2, DiffHunkKind::Modified),
            (4, DiffHunkKind::Deleted),
        ]
    );
    // 新增块默认折叠：只保留 gutter 竖条，不整行着色。
    assert_eq!(rendered.expanded_rows, Vec::<Range<usize>>::new());
    // 纯新增块也要登记点击区域，否则普通文档既不能展开也无法着色。
    assert!(
        rendered
            .hit_regions
            .iter()
            .any(|(_, index, kind)| *index == 0 && *kind == DiffHunkKind::Added),
        "纯新增块应可点击展开"
    );

    // 差异审阅视图（默认展开）下新增块才整行着色。
    let expanded_added = hunk_rendering(
        &snapshot,
        resolved_hunks(&hunks[..1], &[true], &[None], &[]).into_iter(),
    );
    assert_eq!(expanded_added.expanded_rows, vec![0..1]);
}

#[gpui::test]
fn materialized_modified_hunk_uses_real_old_and_new_document_rows(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text("context\nold\nnew\nafter\n".into(), BufferConfig::default())
        .expect("应创建测试 Buffer");
    let snapshot = project_display_snapshot(cx, buffer.snapshot());
    let hunk = DisplayHunk {
        range: 2..3,
        old_range: 10..11,
        kind: DiffHunkKind::Modified,
        staging: DiffHunkStaging::NoStaging,
    };
    let old_ranges = vec![Some(1..2)];

    let rendered = hunk_rendering(
        &snapshot,
        resolved_hunks(std::slice::from_ref(&hunk), &[true], &old_ranges, &[]).into_iter(),
    );

    assert_eq!(
        rendered.diff_rows,
        vec![
            (1..2, DiffHunkKind::Deleted, DiffHunkStaging::NoStaging),
            (2..3, DiffHunkKind::Added, DiffHunkStaging::NoStaging)
        ]
    );
    assert_eq!(
        rendered.strips,
        vec![(1..3, DiffHunkKind::Modified, DiffHunkStaging::NoStaging)]
    );
    assert_eq!(
        rendered.controls,
        vec![(1..3, HunkControlTarget::Diff(hunk))]
    );
    assert_eq!(
        rendered.hit_regions,
        vec![(1..3, 0, DiffHunkKind::Modified)],
        "物化旧侧与普通编辑器共用 gutter 折叠入口"
    );
}

#[gpui::test]
fn word_diff_highlights_only_render_for_expanded_hunks(cx: &mut TestAppContext) {
    // 词级背景只在展开态出现：折叠的修改块没有物化旧侧，也就没有行内变化文本可着色。
    let buffer =
        Buffer::from_text("old\nnew\n".into(), BufferConfig::default()).expect("应创建测试 Buffer");
    let snapshot = project_display_snapshot(cx, buffer.snapshot());
    let hunks = vec![DisplayHunk {
        range: 1..2,
        old_range: 0..1,
        kind: DiffHunkKind::Modified,
        staging: DiffHunkStaging::NoStaging,
    }];
    let word_diffs = vec![vec![
        (DiffHunkKind::Deleted, 0..3),
        (DiffHunkKind::Added, 4..7),
    ]];

    let collapsed = hunk_rendering(
        &snapshot,
        resolved_hunks(&hunks, &[false], &[Some(0..1)], &word_diffs).into_iter(),
    );
    assert_eq!(
        collapsed.word_diff_highlights,
        Vec::<(DiffHunkKind, Range<usize>)>::new()
    );

    let expanded = hunk_rendering(
        &snapshot,
        resolved_hunks(&hunks, &[true], &[Some(0..1)], &word_diffs).into_iter(),
    );
    assert_eq!(expanded.word_diff_highlights, word_diffs[0]);
}

#[gpui::test]
fn staging_drives_hollow_blocks(cx: &mut TestAppContext) {
    // hunk_rendering 把暂存语义透传到行标记与 gutter 竖条，渲染端据此选空心 / 实心。
    let buffer =
        Buffer::from_text("a\nb\nc\n".into(), BufferConfig::default()).expect("应创建测试 Buffer");
    let snapshot = project_display_snapshot(cx, buffer.snapshot());
    let staged = DisplayHunk {
        range: 1..3,
        old_range: 1..3,
        kind: DiffHunkKind::Added,
        staging: DiffHunkStaging::Staged,
    };
    let rendered = hunk_rendering(
        &snapshot,
        resolved_hunks(&[staged], &[true], &[None], &[]).into_iter(),
    );
    assert_eq!(
        rendered.diff_rows,
        vec![(1..3, DiffHunkKind::Added, DiffHunkStaging::Staged)]
    );
    assert_eq!(
        rendered.strips,
        vec![(1..3, DiffHunkKind::Added, DiffHunkStaging::Staged)]
    );
    // 多行 hollow 只产出一个连续块，边框按块边界合并，不会逐行描边叠加。
    assert_eq!(
        rendered.hollow_blocks,
        vec![1..3],
        "多行已暂存 hunk 应合并为单个 hollow 块"
    );
    // 实心（未暂存）不产生任何 hollow 块。
    let unstaged = DisplayHunk {
        range: 1..3,
        old_range: 1..3,
        kind: DiffHunkKind::Added,
        staging: DiffHunkStaging::NoStaging,
    };
    let rendered = hunk_rendering(
        &snapshot,
        resolved_hunks(&[unstaged], &[true], &[None], &[]).into_iter(),
    );
    assert!(rendered.hollow_blocks.is_empty());
}

#[gpui::test]
fn hunk_click_regions_do_not_depend_on_staging(cx: &mut TestAppContext) {
    // 点击展开只由 hunk 类型决定；已暂存 / 未暂存只影响配色，避免形成双轨。
    let buffer =
        Buffer::from_text("a\nb\nc\n".into(), BufferConfig::default()).expect("应创建测试 Buffer");
    let snapshot = project_display_snapshot(cx, buffer.snapshot());
    for staging in [
        DiffHunkStaging::Staged,
        DiffHunkStaging::Unstaged,
        DiffHunkStaging::NoStaging,
    ] {
        let hunk = DisplayHunk {
            range: 0..1,
            old_range: 0..0,
            kind: DiffHunkKind::Added,
            staging,
        };
        let rendered = hunk_rendering(
            &snapshot,
            resolved_hunks(&[hunk], &[true], &[None], &[]).into_iter(),
        );
        assert_eq!(
            rendered.hit_regions,
            vec![(0..1, 0, DiffHunkKind::Added)],
            "点击区域不应随暂存语义变化：{staging:?}"
        );
    }
}
