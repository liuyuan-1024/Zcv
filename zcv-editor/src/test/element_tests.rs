use crate::display_map::{DiffDecorationSnapshot, FoldPlaceholder};
use zcv_multi_buffer::{ResolvedDiffHunk, WordDiffs};

use super::*;

#[cfg(test)]
fn layout_visible_lines(
    display_snapshot: DisplaySnapshot,
    placeholder: Option<DisplaySnapshot>,
    presentation: EditorPresentation,
    search_decorations: Option<&SearchDecorationSnapshot>,
    params: VisibleLineLayoutParams<'_>,
    window: &mut Window,
    cx: &mut App,
) -> EditorLayout {
    let placeholder_mode = placeholder.is_some();
    let display_snapshot = placeholder.as_ref().unwrap_or(&display_snapshot);
    let line_count = display_snapshot.line_count();
    let start = params.start_row.get().min(line_count.saturating_sub(1));
    let visible_count = ((params.geometry.text_bounds.size.height + params.scroll_offset.y)
        / params.line_height)
        .ceil() as usize
        + 1;
    let visible_source_ranges = display_snapshot
        .chunks(
            DisplayRow::new(start)
                ..DisplayRow::new(start + visible_count.min(line_count.saturating_sub(start))),
            HighlightStyles::default(),
            None,
        )
        .source_line_ranges();
    let visible_source_lines = source_ranges_bounds(&visible_source_ranges);
    layout_visible_lines_from_viewport(
        VisibleViewport {
            display_snapshot,
            placeholder_mode,
            visible_source_ranges,
            visible_source_lines,
        },
        presentation,
        search_decorations,
        params,
        window,
        cx,
    )
}

use crate::display_map::test_support::{WrapRowKind, projected_line_text};
use crate::display_map::{DisplayMap, DisplaySnapshot};
use gpui::{AppContext, Empty, TestAppContext};

use std::path::{Path, PathBuf};
use zcv_buffer_diff::DiffHunkStaging;
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{DisplayHunk, ExcerptRange, MultiBuffer};
use zcv_text::{Affinity, Buffer, BufferConfig, Line};
use zcv_theme::{ThemeChoice, typography};

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

#[test]
fn collapsed_deleted_hunk_triangle_is_centered_on_the_deletion_boundary() {
    let bounds = Bounds::from_corners(point(px(0.), px(20.)), point(px(5.), px(60.)));

    assert_eq!(
        deleted_hunk_triangle_points(bounds, px(5.)),
        [
            point(px(0.), px(16.)),
            point(px(0.), px(24.)),
            point(px(5.), px(20.)),
        ]
    );
}

#[gpui::test]
fn search_marker_rows_cover_every_current_search_range(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text("first\nmiddle\n项目".to_owned(), BufferConfig::default())
        .expect("应创建搜索 marker 测试 Buffer");
    let snapshot = buffer.snapshot();
    let display = project_display_snapshot(cx, snapshot.clone());
    let ranges = [
        MultiBufferRange::new(MultiBufferOffset::ZERO, MultiBufferOffset::new(5)).unwrap(),
        MultiBufferRange::new(MultiBufferOffset::new(13), MultiBufferOffset::new(19)).unwrap(),
    ];
    let decorations = SearchDecorationSnapshot::for_test(&display, &ranges, 0);

    assert_eq!(
        decorations.projected_rows_for_test(),
        vec![0..1, 2..3],
        "每个当前搜索范围都应转换为滚动栏 marker 的显示行范围"
    );
}

#[gpui::test]
fn search_scrollbar_markers_only_render_for_singleton_documents(cx: &mut TestAppContext) {
    // 对齐 zed：搜索命中的滚动轴标记只服务单文档编辑器；
    // 组合文档不投影整份命中，避免结果很多时每次重建都做整份显示行投影。
    let text = "first\n项目\nlast";
    let singleton_source = cx.new(|cx| {
        let buffer = Buffer::from_text(text.to_owned(), BufferConfig::default())
            .expect("应创建单文档搜索 marker 测试 Buffer");
        LanguageBuffer::new(
            buffer,
            None,
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let singleton_buffer = cx.new(|cx| MultiBuffer::singleton(singleton_source, cx));
    let singleton_editor = cx.new(|cx| Editor::for_multi_buffer(singleton_buffer, cx));

    let combined_source = cx.new(|cx| {
        let buffer = Buffer::from_text(text.to_owned(), BufferConfig::default())
            .expect("应创建组合文档搜索 marker 测试 Buffer");
        LanguageBuffer::new(
            buffer,
            None,
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let combined_buffer = cx.new(MultiBuffer::empty);
    let source_len = cx.read_entity(&combined_source, |source, _| {
        source.text_snapshot().len_bytes()
    });
    combined_buffer.update(cx, |buffer, cx| {
        buffer.set_excerpts_for_path(
            vec![ExcerptRange::new(
                combined_source.clone(),
                zcv_text::TextRange::new(zcv_text::ByteOffset::ZERO, source_len)
                    .expect("整文件范围应合法"),
                Vec::new(),
            )],
            cx,
        );
    });
    let combined_editor = cx.new(|cx| Editor::for_multi_buffer(combined_buffer, cx));

    assert!(
        cx.read_entity(&singleton_editor, |editor, cx| {
            editor.shows_search_scrollbar_markers(cx)
        }),
        "单文档编辑器应渲染搜索 marker"
    );
    assert!(
        !cx.read_entity(&combined_editor, |editor, cx| {
            editor.shows_search_scrollbar_markers(cx)
        }),
        "组合文档不应投影搜索 marker"
    );
}

/// 测试专用：以完整视口构建 diff 行渲染数据。
///
/// 走生产入口 `DiffDecorationSnapshot::new`，不另外暴露内部 `hunk_rendering`。
fn test_hunk_rendering(
    snapshot: &DisplaySnapshot,
    hunks: &[DisplayHunk],
    expanded: &[bool],
    old_display_ranges: &[Option<Range<usize>>],
) -> DiffDecorationSnapshot {
    let resolved: Vec<ResolvedDiffHunk> = hunks
        .iter()
        .enumerate()
        .map(|(index, hunk)| ResolvedDiffHunk {
            hunk: hunk.clone(),
            old_range: old_display_ranges.get(index).cloned().flatten(),
            expanded: expanded.get(index).copied().unwrap_or(false),
            word_diffs: WordDiffs::default(),
        })
        .collect();
    DiffDecorationSnapshot::from_resolved(snapshot, resolved, &[])
}

/// 行级标记的显示行区间（测试专用）。
fn diff_hunk_rows(
    snapshot: &DisplaySnapshot,
    hunks: &[DisplayHunk],
    expanded: &[bool],
    old_display_ranges: &[Option<Range<usize>>],
) -> Vec<(Range<usize>, DiffHunkKind)> {
    test_hunk_rendering(snapshot, hunks, expanded, old_display_ranges)
        .rendering_for_viewport(0..usize::MAX)
        .diff_rows
        .into_iter()
        .map(|(rows, kind, _)| (rows, kind))
        .collect()
}

/// hunk 竖条范围与状态色（测试专用）。
fn hunk_strip_rows(
    snapshot: &DisplaySnapshot,
    hunks: &[DisplayHunk],
    expanded: &[bool],
    old_display_ranges: &[Option<Range<usize>>],
) -> Vec<(Range<usize>, DiffHunkKind)> {
    test_hunk_rendering(snapshot, hunks, expanded, old_display_ranges)
        .rendering_for_viewport(0..usize::MAX)
        .strips
        .into_iter()
        .map(|(rows, kind, _)| (rows, kind))
        .collect()
}

/// 可点击的 hunk 色带区域（测试专用）。
fn hunk_hit_regions(
    snapshot: &DisplaySnapshot,
    hunks: &[DisplayHunk],
    expanded: &[bool],
    old_display_ranges: &[Option<Range<usize>>],
) -> Vec<(Range<usize>, usize, DiffHunkKind)> {
    test_hunk_rendering(snapshot, hunks, expanded, old_display_ranges)
        .rendering_for_viewport(0..usize::MAX)
        .hit_regions
}

/// 回归：run 背景（搜索高亮等）片段必须叠加行原点 x。
///
/// 背景片段与选区片段同处窗口绝对坐标；
/// run 背景的字节区间是 shaped 文本内偏移，只取 `x_for_index` 而不加 `line.origin.x` 会把高亮整体左移（漏掉文本区起点偏移）。
#[gpui::test]
fn background_fragments_include_line_origin_x(cx: &mut TestAppContext) {
    let text = "代码 abc 代码\n";
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("应创建 Buffer");
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("README.md")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    cx.run_until_parked();
    let snapshot = cx.read_entity(&language_buffer, |language_buffer, _| {
        language_buffer.text_snapshot()
    });
    let multi_buffer = cx.new(|cx| MultiBuffer::singleton(language_buffer, cx));
    cx.run_until_parked();
    let multi_snapshot =
        cx.update_entity(&multi_buffer, |multi_buffer, cx| multi_buffer.snapshot(cx));

    let ranges = vec![
        MultiBufferRange::new(MultiBufferOffset::new(0), MultiBufferOffset::new(6)).unwrap(),
        MultiBufferRange::new(MultiBufferOffset::new(11), MultiBufferOffset::new(17)).unwrap(),
    ];
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let map = new_display_map(cx, multi_snapshot.clone());
            let display = cx.update_entity(&map, |map, cx| map.snapshot(cx));
            let search_decorations =
                SearchDecorationSnapshot::for_test(&display, &ranges, 0);
            // 文本区起点 = 60px（真实编辑器带 gutter 时的典型偏移）。
            let text_origin_x = px(60.);
            let layout = layout_visible_lines(
                display,
                None,
                EditorPresentation::new(&snapshot.clone().into(), None),
                Some(&search_decorations),
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(
                            point(text_origin_x, px(0.)),
                            size(px(700.), px(80.)),
                        ),
                        text_clip_bounds: Bounds::new(
                            point(text_origin_x, px(0.)),
                            size(px(700.), px(80.)),
                        ),
                        gutter: None,
                    },
                    active_lines: &BTreeSet::new(),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::new(0),
                    scroll_offset: point(px(0.), px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );
            assert_eq!(layout.lines[0].row, DisplayRow::new(0));
            let line = &layout.lines[0];
            assert_eq!(line.background_runs.len(), 2, "两个匹配都应进入背景层");
            // 每个 run 背景的片段像素区间必须与“行原点 + 字形偏移”一致。
            let fragments = layout_line_background_fragments(line, &[], gpui::rgba(0xff0000ff));
            assert_eq!(fragments.len(), line.background_runs.len());
            for (fragment, (byte_range, _)) in fragments.iter().zip(&line.background_runs) {
                let expected_start =
                    line.origin.x + line.line.x_for_index(byte_range.start);
                let expected_end = line.origin.x + line.line.x_for_index(byte_range.end);
                assert!(
                    (fragment.start_x - expected_start).abs() < px(1.)
                        && (fragment.end_x - expected_end).abs() < px(1.),
                    "run 背景片段必须与文本对齐：片段 {fragment:?}，期望 {expected_start:?}..{expected_end:?}"
                );
            }
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn active_line_background_covers_all_soft_wrapped_rows(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let text = "aaaa bbbb cccc dddd eeee ".repeat(3) + "\nshort\n";
            let snapshot = Buffer::from_text(text, BufferConfig::default())
                .expect("测试 Buffer 应能创建")
                .snapshot();
            let text_style = window.text_style();
            let font_size = text_style.font_size.to_pixels(window.rem_size());
            let map = new_display_map(cx, snapshot.clone());
            cx.update_entity(&map, |map, cx| {
                map.set_wrap_width(
                    Some(px(100.)),
                    text_style.font(),
                    font_size,
                    window.text_system(),
                    cx,
                )
            });
            let display = cx.update_entity(&map, |map, cx| map.snapshot(cx));
            let dimensions = gutter_dimensions(&display, window);
            let gutter_bounds =
                Bounds::new(point(px(0.), px(0.)), size(dimensions.width, px(200.)));
            let active_lines = BTreeSet::from([Line::ZERO]);
            let layout = layout_visible_lines(
                display,
                None,
                EditorPresentation::new(&snapshot.clone().into(), None),
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(
                            point(gutter_bounds.right(), px(0.)),
                            size(px(340.), px(200.)),
                        ),
                        text_clip_bounds: Bounds::new(
                            point(gutter_bounds.right(), px(0.)),
                            size(px(340.), px(200.)),
                        ),
                        gutter: Some((gutter_bounds, dimensions)),
                    },
                    active_lines: &active_lines,
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::ZERO,
                    scroll_offset: point(px(0.), px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );

            let wrapped_rows = layout
                .lines
                .iter()
                .filter(|line| line.logical_line == Some(Line::ZERO))
                .collect::<Vec<_>>();
            assert!(wrapped_rows.len() > 1, "测试行应拆成多个软换行显示行");
            assert!(wrapped_rows.iter().all(|line| line.active));
            assert!(
                layout
                    .lines
                    .iter()
                    .any(|line| line.logical_line == Some(Line::new(1)) && !line.active),
                "下一逻辑行不应继承活动状态"
            );

            let active_bounds = layout
                .active_line_background_bounds(
                    gutter_bounds.left(),
                    gutter_bounds.right() + px(340.),
                )
                .collect::<Vec<_>>();
            assert_eq!(active_bounds.len(), wrapped_rows.len());
            assert!(
                active_bounds
                    .windows(2)
                    .all(|bounds| { bounds[1].top() == bounds[0].bottom() })
            );
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn editor_width_soft_wrap_keeps_mixed_cjk_inside_text_bounds(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let font = typography::ui_font();
            let font_size = typography::ui_size(cx);
            let font_id = window.text_system().resolve_font(&font);
            let em_advance = window
                .text_system()
                .em_advance(font_id, font_size)
                .expect("UI 字体必须包含拉丁字形");
            for text in [
                "新增 Zcv 架构维护技能并整合可见性清理、架构体检与架构减法流程",
                "补全各语言的括号、缩进、折叠与注入查询文件",
                "修复 SVG 与 Markdown 公式预览的缩放、居中、清晰度、颜色及边界裁剪问题",
            ] {
                let snapshot = Buffer::from_text(text.to_owned(), BufferConfig::default())
                    .expect("测试 Buffer 应能创建")
                    .snapshot();
                let map = new_display_map(cx, snapshot);
                for quarter_pixels in 720..=2_400 {
                    let text_width = px(quarter_pixels as f32 / 4.);
                    let text_layout_width = (text_width
                        - wrap_edge_safety(SoftWrap::EditorWidth, em_advance))
                    .max(Pixels::ZERO);
                    let wrap_width = calculate_wrap_width(
                        SoftWrap::EditorWidth,
                        text_layout_width,
                        80,
                        em_advance,
                    );
                    cx.update_entity(&map, |map, cx| map.set_wrap_width(wrap_width, font.clone(), font_size, window.text_system(), cx));
                    let display = cx.update_entity(&map, |map, cx| map.snapshot(cx));
                    let mut cursor = display.rows(DisplayRow::ZERO, display.line_count());
                    let viewport: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
                    if text.starts_with("新增") && quarter_pixels == 720 {
                        let rows: Vec<_> = viewport
                            .iter()
                            .map(|row| {
                                let WrapRowKind::Text {
                                    byte_range,
                                    projected_line,
                                    ..
                                } = row.kind();
                                projected_line_text(&display, *projected_line)
                                    .expect("显示行文本应可解析")
                                    .as_ref()[byte_range.clone()]
                                    .to_owned()
                            })
                            .collect();
                        assert_eq!(
                            rows,
                            [
                                "新增 Zcv 架构维护技能并整合可见性清理",
                                "、架构体检与架构减法流程",
                            ]
                        );
                    }

                    for row in &viewport {
                        let WrapRowKind::Text {
                            byte_range,
                            projected_line,
                            ..
                        } = row.kind();
                        let row_text = projected_line_text(&display, *projected_line)
                            .expect("显示行文本应可解析");
                        let row_text = &row_text.as_ref()[byte_range.clone()];
                        let run = TextRun {
                            len: row_text.len(),
                            font: font.clone(),
                            color: gpui::black(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let shaped = window.text_system().shape_line(
                            row_text.to_owned().into(),
                            font_size,
                            &[run],
                            None,
                        );
                        assert!(
                            shaped.width <= text_layout_width,
                            "软换行行尾不应超出文本布局区：layout_width={}, shaped={}，row={row_text:?}",
                            f32::from(text_layout_width),
                            f32::from(shaped.width),
                        );
                    }
                }
            }
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn multibuffer_header_can_start_above_viewport(cx: &mut TestAppContext) {
    let text = "引擎\n";
    let source_text =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("应创建源 Buffer");
    let source = cx.new({
        move |cx| {
            LanguageBuffer::new(
                source_text,
                Some(PathBuf::from("文档/引擎.md")),
                std::sync::Arc::new(LanguageRegistry::new()),
                cx,
            )
        }
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |combined, cx| {
        combined.set_excerpts_for_path(
            vec![
                ExcerptRange::new(
                    source,
                    MultiBufferRange::new(
                        MultiBufferOffset::ZERO,
                        MultiBufferOffset::new(text.len()),
                    )
                    .expect("片段范围应有效")
                    .into(),
                    Vec::new(),
                )
                .with_display_path(PathBuf::from("文档/引擎.md")),
            ],
            cx,
        );
    });
    cx.run_until_parked();

    let multi_snapshot = cx.update_entity(&combined, |combined, cx| combined.snapshot(cx));
    let text_snapshot = multi_snapshot.clone();
    let display_snapshot = project_display_snapshot(cx, multi_snapshot);
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let dimensions = GutterDimensions {
                crease_width: px(8.),
                left_padding: px(8.),
                right_padding: px(8.),
                width: px(56.),
                margin: px(3.),
            };
            let gutter_bounds = Bounds::new(point(px(0.), px(0.)), size(dimensions.width, px(80.)));
            // 文件标题占两行。视口从标题的第二行开始时，标题真实起点在
            // viewport 上方一行；这是合法的负布局偏移，不能用 usize 相减。
            let layout = layout_visible_lines(
                display_snapshot,
                None,
                EditorPresentation::new(&text_snapshot, None),
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(point(px(59.), px(0.)), size(px(341.), px(80.))),
                        text_clip_bounds: Bounds::new(
                            point(px(56.), px(0.)),
                            size(px(344.), px(80.)),
                        ),
                        gutter: Some((gutter_bounds, dimensions)),
                    },
                    active_lines: &BTreeSet::new(),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::new(1),
                    scroll_offset: point(px(0.), px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );

            assert_eq!(layout.blocks.len(), 1);
            assert_eq!(layout.blocks[0].row, DisplayRow::ZERO);
            assert_eq!(layout.blocks[0].origin.x, px(0.));
            assert_eq!(layout.blocks[0].origin.y, px(-20.));
            assert_eq!(layout.block_clip_bounds.size.width, px(400.));
            assert_eq!(layout.lines[0].line.text.as_str(), "引擎");
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn sticky_buffer_header_follows_excerpts_and_points_to_the_next_file(cx: &mut TestAppContext) {
    let first_text = "a0\na1\na2\na3\na4\na5\na6\na7\n";
    let first_buffer = Buffer::from_text(first_text.to_owned(), BufferConfig::default())
        .expect("应创建第一个源 Buffer");
    let first = cx.new(move |cx| {
        LanguageBuffer::new(
            first_buffer,
            Some(PathBuf::from("src/a.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });

    let second_text = "b0\nb1\n";
    let second_buffer = Buffer::from_text(second_text.to_owned(), BufferConfig::default())
        .expect("应创建第二个源 Buffer");
    let second = cx.new(move |cx| {
        LanguageBuffer::new(
            second_buffer,
            Some(PathBuf::from("src/b.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });

    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |combined, cx| {
        combined.set_excerpts_for_path(
            vec![
                ExcerptRange::line_range(first.clone(), 0..2, cx),
                ExcerptRange::line_range(first, 5..7, cx),
            ],
            cx,
        );
        combined.set_excerpts_for_path(vec![ExcerptRange::line_range(second, 0..2, cx)], cx);
    });
    cx.run_until_parked();

    let snapshot = cx.update_entity(&combined, |combined, cx| combined.snapshot(cx));
    let display = project_display_snapshot(cx, snapshot);
    let mut cursor = display.rows(DisplayRow::ZERO, display.line_count());
    let rows: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
    let blocks = rows
        .iter()
        .filter_map(|row| {
            row.block()
                .map(|block| (row.index(), block.kind, block.excerpt.source_start_line()))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        blocks.iter().map(|(_, kind, _)| *kind).collect::<Vec<_>>(),
        vec![
            DisplayBlockKind::BufferHeader,
            DisplayBlockKind::ExcerptBoundary,
            DisplayBlockKind::BufferHeader,
        ]
    );

    let first_header = display
        .sticky_buffer_header(DisplayRow::new(blocks[0].0.get() + FILE_HEADER_HEIGHT))
        .expect("第一个文件正文应有悬浮标题");
    assert_eq!(first_header.excerpt.path(), Path::new("src/a.rs"));
    assert_eq!(first_header.excerpt.source_start_line(), 1);

    let later_excerpt = display
        .sticky_buffer_header(blocks[1].0)
        .expect("同文件后续 excerpt 应更新悬浮标题目标");
    assert_eq!(later_excerpt.excerpt.path(), Path::new("src/a.rs"));
    assert_eq!(later_excerpt.excerpt.source_start_line(), 6);
    assert_eq!(later_excerpt.next_buffer_header_row, Some(blocks[2].0));

    let second_header = display
        .sticky_buffer_header(blocks[2].0)
        .expect("到达下一个文件后应切换悬浮标题");
    assert_eq!(second_header.excerpt.path(), Path::new("src/b.rs"));
    assert_eq!(second_header.next_buffer_header_row, None);
}

#[test]
fn next_file_header_pushes_sticky_header_out() {
    let start = DisplayRow::new(10);
    let next = Some(DisplayRow::new(12));
    assert_eq!(
        sticky_buffer_header_origin_y(px(100.), px(20.), start, px(0.), next),
        px(100.)
    );
    assert_eq!(
        sticky_buffer_header_origin_y(px(100.), px(20.), start, px(10.), next),
        px(90.)
    );
    assert_eq!(
        sticky_buffer_header_origin_y(px(100.), px(20.), start, px(10.), None),
        px(100.)
    );
}

#[gpui::test]
fn wrapped_unicode_markdown_queries_highlights_from_source_chunks(cx: &mut TestAppContext) {
    let text = "> **The reconstructed Functionally Equivalent Scene（功能等价场景）can be directly imported into ROS（机器人操作系统）to support interactive simulation（交互式仿真）and long-horizon robot task execution（长时序机器人任务执行）.**\n";
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("应创建 Buffer");
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("README.md")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    cx.run_until_parked();
    let snapshot = cx.read_entity(&language_buffer, |language_buffer, _| {
        language_buffer.text_snapshot()
    });
    let multi_buffer = cx.new(|cx| MultiBuffer::singleton(language_buffer, cx));
    cx.run_until_parked();
    let multi_snapshot =
        cx.update_entity(&multi_buffer, |multi_buffer, cx| multi_buffer.snapshot(cx));
    assert!(
        !multi_snapshot.highlights(0..text.len()).is_empty(),
        "组合文档应从源片段查询 Markdown 高亮"
    );

    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let text_system = window.text_system().clone();
            let font = window.text_style().font();
            let font_size = window.text_style().font_size.to_pixels(window.rem_size());
            let map = new_display_map(cx, multi_snapshot.clone());

            // 选择一个“片段长度不是行首 UTF-8 边界”的续行。旧实现把这个长度
            // 拼到行首上查询高亮，正好会制造落在中文编码内部的 capture 端点。
            let mut offending_row = None;
            for width in [px(320.), px(400.), px(480.), px(560.), px(640.)] {
                cx.update_entity(&map, |map, cx| {
                    map.set_wrap_width(Some(width), font.clone(), font_size, &text_system, cx)
                });
                let display = cx.update_entity(&map, |map, cx| map.snapshot(cx));
                let mut cursor = display.rows(DisplayRow::ZERO, display.line_count());
                let viewport: Vec<_> = std::iter::from_fn(|| cursor.next()).collect();
                offending_row = viewport.iter().find_map(|row| {
                    let WrapRowKind::Text {
                        byte_range,
                        projected_line,
                        ..
                    } = row.kind();
                    let text =
                        projected_line_text(&display, *projected_line).expect("显示行文本应可解析");
                    (!text.is_char_boundary(byte_range.len())).then_some(row.index())
                });
                if offending_row.is_some() {
                    break;
                }
            }
            let offending_row = offending_row.expect("测试文本应产生目标 UTF-8 续行");
            let display = cx.update_entity(&map, |map, cx| map.snapshot(cx));
            let layout = layout_visible_lines(
                display,
                None,
                EditorPresentation::new(&snapshot.clone().into(), None),
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(point(px(0.), px(0.)), size(px(700.), px(80.))),
                        text_clip_bounds: Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(700.), px(80.)),
                        ),
                        gutter: None,
                    },
                    active_lines: &BTreeSet::new(),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: offending_row,
                    scroll_offset: point(px(0.), px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );
            assert_eq!(
                layout.lines.first().map(|line| line.row),
                Some(offending_row)
            );
            assert!(!layout.lines[0].line.text.is_empty());
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn large_buffer_layout_shapes_only_visible_rows(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let text = (0..10_000)
                .map(|row| format!("line {row}\n"))
                .collect::<String>();
            let snapshot = Buffer::from_text(text, BufferConfig::default())
                .expect("大文本测试 Buffer 应能创建")
                .snapshot();
            let presentation = EditorPresentation::new(&snapshot.clone().into(), None);
            let display_snapshot = project_display_snapshot(cx, snapshot.clone());
            let layout = layout_visible_lines(
                display_snapshot,
                None,
                presentation,
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(point(px(0.), px(0.)), size(px(800.), px(100.))),
                        text_clip_bounds: Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(800.), px(100.)),
                        ),
                        gutter: None,
                    },
                    active_lines: &BTreeSet::new(),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::new(5_000),
                    scroll_offset: point(px(0.), px(10.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );

            assert_eq!(
                layout.lines.first().map(|line| line.row),
                Some(DisplayRow::new(5_000))
            );
            assert_eq!(
                layout.lines.last().map(|line| line.row),
                Some(DisplayRow::new(5_006))
            );
            assert_eq!(layout.lines.len(), 7);
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn gutter_and_text_share_vertical_rows_but_only_text_scrolls_horizontally(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let snapshot = Buffer::from_text("one\ntwo\nthree".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建")
                .snapshot();
            let dimensions = GutterDimensions {
                crease_width: px(8.),
                left_padding: px(8.),
                right_padding: px(8.),
                width: px(56.),
                margin: px(3.),
            };
            let gutter_bounds =
                Bounds::new(point(px(0.), px(0.)), size(dimensions.width, px(100.)));
            let text_bounds = Bounds::new(point(px(59.), px(0.)), size(px(341.), px(100.)));
            let layout = layout_visible_lines(
                project_display_snapshot(cx, snapshot.clone()),
                None,
                EditorPresentation::new(&snapshot.clone().into(), None),
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds,
                        text_clip_bounds: Bounds::new(
                            point(px(56.), px(0.)),
                            size(px(344.), px(100.)),
                        ),
                        gutter: Some((gutter_bounds, dimensions)),
                    },
                    active_lines: &BTreeSet::from([Line::new(1)]),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::ZERO,
                    scroll_offset: point(px(20.), px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );
            let gutter = layout.gutter.as_ref().expect("Full Editor 应布局 gutter");

            assert_eq!(layout.lines[0].origin.x, px(39.));
            assert_eq!(gutter.rows[0].shaped_line_number.text.as_ref(), "1");
            assert_eq!(gutter.rows[1].shaped_line_number.text.as_ref(), "2");
            assert!(!layout.lines[0].active);
            assert!(layout.lines[1].active);
            assert_eq!(gutter.rows[0].origin.y, layout.lines[0].origin.y);
            assert!(gutter.rows[0].origin.x > gutter_bounds.left());
            assert_eq!(layout.text_clip_bounds.left(), gutter_bounds.right());
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn folded_projection_rows_drive_layout_and_hit_testing(cx: &mut TestAppContext) {
    let snapshot = Buffer::from_text(
        "anchor\nhidden one\nhidden two\nafter".to_owned(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建")
    .snapshot();
    let map = new_display_map(cx, snapshot.clone());
    cx.update_entity(&map, |map, cx| {
        let range = {
            let display = map.snapshot(cx);
            let snapshot = display.buffer_snapshot();
            snapshot.anchor_at(MultiBufferOffset::new(6), Affinity::Before)
                ..snapshot.anchor_at(MultiBufferOffset::new(28), Affinity::After)
        };
        map.fold_range(range, FoldPlaceholder::default(), cx)
    })
    .expect("折叠应成功");

    // 行布局会测量折叠占位符元素，必须发生在真实的 request_layout/prepaint 生命周期内；
    // 断言在生命周期内完成，避免持有元素超出其 arena 生命周期。
    let visual = cx.add_empty_window();
    visual.draw(
        point(px(0.), px(0.)),
        size(px(400.), px(100.)),
        |window, cx| {
            let layout = layout_visible_lines(
                map.update(cx, |map, cx| map.snapshot(cx)),
                None,
                EditorPresentation::new(&snapshot.clone().into(), None),
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(100.))),
                        text_clip_bounds: Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(400.), px(100.)),
                        ),
                        gutter: None,
                    },
                    active_lines: &BTreeSet::new(),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::ZERO,
                    scroll_offset: point(px(0.), px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );

            assert_eq!(layout.lines.len(), 2);
            // 折叠合并行：anchor 文本 + 占位符拼成同一显示行。
            assert_eq!(layout.lines[0].line.text.as_str(), "anchor⋯");
            assert_eq!(layout.lines[1].line.text.as_str(), "after");
            Empty
        },
    );
}

/// 折叠占位符是真正的行内元素片段：跨片段坐标与命中必须穿过文本→元素→文本，
/// 落在元素内部的索引吸附到元素起点，元素之后回到元素终点。
#[gpui::test]
fn folded_element_participates_in_cross_fragment_coordinates(cx: &mut TestAppContext) {
    let snapshot = Buffer::from_text(
        "anchor\nhidden one\nafter".to_owned(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建")
    .snapshot();
    let map = new_display_map(cx, snapshot.clone());
    let placeholder = FoldPlaceholder {
        render: std::sync::Arc::new(|_, _, _| div().w(px(30.)).h(px(10.)).into_any_element()),
        constrain_width: false,
        ..FoldPlaceholder::default()
    };
    cx.update_entity(&map, |map, cx| {
        let range = {
            let display = map.snapshot(cx);
            let snapshot = display.buffer_snapshot();
            snapshot.anchor_at(MultiBufferOffset::new(6), Affinity::Before)
                ..snapshot.anchor_at(MultiBufferOffset::new(17), Affinity::After)
        };
        map.fold_range(range, placeholder, cx)
    })
    .expect("折叠应成功");

    let visual = cx.add_empty_window();
    visual.draw(
        point(px(0.), px(0.)),
        size(px(400.), px(100.)),
        |window, cx| {
            let layout = layout_visible_lines(
                map.update(cx, |map, cx| map.snapshot(cx)),
                None,
                EditorPresentation::new(&snapshot.clone().into(), None),
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(100.))),
                        text_clip_bounds: Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(400.), px(100.)),
                        ),
                        gutter: None,
                    },
                    active_lines: &BTreeSet::new(),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::ZERO,
                    scroll_offset: point(px(0.), px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );

            let line = &layout.lines[0].line;
            assert_eq!(line.text.as_str(), "anchor⋯");
            assert!(
                matches!(line.fragments.first(), Some(LineFragment::Text(_))),
                "折叠行必须以文本片段开头"
            );
            let (element_size, element_len) = line
                .fragments
                .iter()
                .find_map(|fragment| match fragment {
                    LineFragment::Element { size, len, .. } => Some((*size, *len)),
                    LineFragment::Text(_) => None,
                })
                .expect("折叠占位符必须布局成元素片段");
            assert_eq!(element_len, "⋯".len());
            assert_eq!(element_size.width, px(30.));

            // 完整行索引空间包含被替换文本：元素起点仍对应折叠范围的起始字节。
            let element_start = line.x_for_index(6);
            assert_eq!(
                line.x_for_index(6 + element_len - 1),
                element_start,
                "元素内部索引必须吸附到元素起点"
            );
            assert_eq!(
                line.x_for_index(6 + element_len),
                element_start + element_size.width,
                "元素之后必须回到元素终点"
            );
            assert_eq!(
                line.closest_index_for_x(element_start + px(5.)),
                6,
                "元素区间命中必须返回元素边界"
            );
            for index in 0..6 {
                assert_eq!(
                    line.closest_index_for_x(line.x_for_index(index)),
                    index,
                    "文本片段内的索引与 x 必须往返一致"
                );
            }
            Empty
        },
    );
}

/// 记录 gpui 布局阶段的可用宽度与 prepaint 原点的测试元素。
struct RecordingElement {
    available: std::sync::Arc<std::sync::Mutex<Option<Size<AvailableSpace>>>>,
    prepaint_origin: std::sync::Arc<std::sync::Mutex<Option<Point<Pixels>>>>,
}

impl Element for RecordingElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, ()) {
        let available = self.available.clone();
        let layout_id =
            window.request_measured_layout(Style::default(), move |_known, space, _window, _cx| {
                *available.lock().expect("可用宽度记录锁不能中毒") = Some(space);
                size(px(10.), px(10.))
            });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
        *self
            .prepaint_origin
            .lock()
            .expect("prepaint 原点记录锁不能中毒") = Some(bounds.origin);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

impl IntoElement for RecordingElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// constrain_width 决定元素收到的可用宽度：为真时等于占位符文本塑形宽度，
/// 为假时按内容测量（MinContent），由渲染层交给元素布局消费。
#[gpui::test]
fn constrain_width_bounds_element_fragment(cx: &mut TestAppContext) {
    let mut available = [None; 2];
    for (constrain_width, slot) in [(true, 0usize), (false, 1usize)] {
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(None));
        let snapshot = Buffer::from_text(
            "anchor\nhidden one\nafter".to_owned(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建")
        .snapshot();
        let map = new_display_map(cx, snapshot.clone());
        let placeholder = FoldPlaceholder {
            render: {
                let recorded = recorded.clone();
                std::sync::Arc::new(move |_, _, _| {
                    RecordingElement {
                        available: recorded.clone(),
                        prepaint_origin: std::sync::Arc::new(std::sync::Mutex::new(None)),
                    }
                    .into_any_element()
                })
            },
            constrain_width,
            ..FoldPlaceholder::default()
        };
        cx.update_entity(&map, |map, cx| {
            let range = {
                let display = map.snapshot(cx);
                let snapshot = display.buffer_snapshot();
                snapshot.anchor_at(MultiBufferOffset::new(6), Affinity::Before)
                    ..snapshot.anchor_at(MultiBufferOffset::new(17), Affinity::After)
            };
            map.fold_range(range, placeholder, cx)
        })
        .expect("折叠应成功");

        let visual = cx.add_empty_window();
        visual.draw(
            point(px(0.), px(0.)),
            size(px(400.), px(100.)),
            |window, cx| {
                layout_visible_lines(
                    map.update(cx, |map, cx| map.snapshot(cx)),
                    None,
                    EditorPresentation::new(&snapshot.clone().into(), None),
                    None,
                    VisibleLineLayoutParams {
                        geometry: EditorGeometry {
                            text_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(400.), px(100.)),
                            ),
                            text_clip_bounds: Bounds::new(
                                point(px(0.), px(0.)),
                                size(px(400.), px(100.)),
                            ),
                            gutter: None,
                        },
                        active_lines: &BTreeSet::new(),
                        foldable_lines: &BTreeSet::new(),
                        fold_anchor_lines: &BTreeSet::new(),
                        start_row: DisplayRow::ZERO,
                        scroll_offset: point(px(0.), px(0.)),
                        primary_caret_column: None,
                        line_height: px(20.),
                        diff_rows: &[],
                    },
                    window,
                    cx,
                );
                Empty
            },
        );
        available[slot] = recorded.lock().expect("可用宽度记录锁不能中毒").take();
    }

    assert!(
        matches!(
            available[0].expect("元素必须被布局").width,
            AvailableSpace::Definite(_)
        ),
        "constrain_width 为真时必须传入占位符文本塑形宽度"
    );
    assert!(
        matches!(
            available[1].expect("元素必须被布局").width,
            AvailableSpace::MinContent
        ),
        "constrain_width 为假时必须按内容测量"
    );
}

/// 行内元素必须在行原点（含水平自动滚动平移）最终确定后 prepaint，
/// 平移布局后 prepaint 原点跟随行原点。
#[gpui::test]
fn inline_element_prepaints_at_the_final_translated_origin(cx: &mut TestAppContext) {
    let recorded = std::sync::Arc::new(std::sync::Mutex::new(None));
    let snapshot = Buffer::from_text(
        "anchor\nhidden one\nafter".to_owned(),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建")
    .snapshot();
    let map = new_display_map(cx, snapshot.clone());
    let placeholder = FoldPlaceholder {
        render: {
            let recorded = recorded.clone();
            std::sync::Arc::new(move |_, _, _| {
                RecordingElement {
                    available: std::sync::Arc::new(std::sync::Mutex::new(None)),
                    prepaint_origin: recorded.clone(),
                }
                .into_any_element()
            })
        },
        ..FoldPlaceholder::default()
    };
    cx.update_entity(&map, |map, cx| {
        let range = {
            let display = map.snapshot(cx);
            let snapshot = display.buffer_snapshot();
            snapshot.anchor_at(MultiBufferOffset::new(6), Affinity::Before)
                ..snapshot.anchor_at(MultiBufferOffset::new(17), Affinity::After)
        };
        map.fold_range(range, placeholder, cx)
    })
    .expect("折叠应成功");

    let mut baseline = None;
    let visual = cx.add_empty_window();
    visual.draw(
        point(px(0.), px(0.)),
        size(px(400.), px(100.)),
        |window, cx| {
            let mut layout = layout_visible_lines(
                map.update(cx, |map, cx| map.snapshot(cx)),
                None,
                EditorPresentation::new(&snapshot.clone().into(), None),
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(100.))),
                        text_clip_bounds: Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(400.), px(100.)),
                        ),
                        gutter: None,
                    },
                    active_lines: &BTreeSet::new(),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::ZERO,
                    scroll_offset: point(px(0.), px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );
            baseline = Some(layout.lines[0].origin.x + layout.lines[0].line.x_for_index(6));
            let delta = point(px(-40.), Pixels::ZERO);
            layout.translate(delta);
            layout.prepaint_line_elements(window, cx);
            Empty
        },
    );

    let baseline = baseline.expect("行布局必须执行");
    let origin = recorded
        .lock()
        .expect("prepaint 原点记录锁不能中毒")
        .expect("折叠元素必须 prepaint");
    // 元素起点来自塑形宽度累加，允许亚像素差异。
    let moved = origin.x - baseline;
    assert!(
        moved > px(-41.) && moved < px(-39.),
        "元素 prepaint 原点必须跟随行原点的水平平移，实际位移 {moved:?}"
    );
}

#[gpui::test]
fn multi_line_selection_uses_one_rounded_contour_with_inner_turns(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let snapshot =
                Buffer::from_text("abcdef\nx\nabcde".to_owned(), BufferConfig::default())
                    .expect("测试 Buffer 应能创建")
                    .snapshot();
            let layout = layout_visible_lines(
                project_display_snapshot(cx, snapshot.clone()),
                None,
                EditorPresentation::new(&snapshot.clone().into(), None),
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(100.))),
                        text_clip_bounds: Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(400.), px(100.)),
                        ),
                        gutter: None,
                    },
                    active_lines: &BTreeSet::new(),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::ZERO,
                    scroll_offset: point(px(0.), px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );
            let selections = SelectionSet::new(vec![crate::selection::Selection::new(
                MultiBufferOffset::new(2),
                MultiBufferOffset::new(12),
            )]);
            let (segments, _) = layout_selections(&selections, &layout, px(20.), cx);
            let segments = segments
                .into_iter()
                .filter(|line| !line.is_empty())
                .map(|mut line| line.remove(0))
                .collect::<Vec<_>>();

            assert_eq!(segments.len(), 3, "连续多行选区应生成三条行片段");
            // 首行携带顶角圆角，末行携带底角圆角，宽度封顶于圆角半径（20 × 0.15）。
            assert_eq!(segments[0].corners[TOP_LEFT].style, CornerStyle::Round);
            assert_eq!(segments[0].corners[TOP_LEFT].width, px(3.));
            assert_eq!(segments[2].corners[BOTTOM_RIGHT].style, CornerStyle::Round);
            assert!(
                segments[0].start_x > segments[1].start_x,
                "首行左边界转入后续行时应形成内凹倒圆角"
            );
            assert!(
                segments[0].end_x > segments[1].end_x && segments[2].end_x > segments[1].end_x,
                "相邻行宽度收缩与扩张应形成两种圆角转折"
            );
            let first_line_width = layout.lines[0].line.x_for_index(layout.lines[0].line.len());
            assert_eq!(
                segments[0].end_x,
                layout.lines[0].origin.x + first_line_width + px(6.),
                "非末行应延伸两个圆角半径"
            );
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn selected_spaces_render_one_dot_each_without_treating_tabs_as_spaces(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let snapshot = Buffer::from_text("a  b\tc".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建")
                .snapshot();
            let layout = layout_visible_lines(
                project_display_snapshot(cx, snapshot.clone()),
                None,
                EditorPresentation::new(&snapshot.clone().into(), None),
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(40.))),
                        text_clip_bounds: Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(400.), px(40.)),
                        ),
                        gutter: None,
                    },
                    active_lines: &BTreeSet::new(),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::ZERO,
                    scroll_offset: point(px(0.), px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );

            assert_eq!(layout.lines[0].whitespaces.len(), 2);
            let selected_spaces = SelectionSet::new(vec![crate::selection::Selection::new(
                MultiBufferOffset::new(1),
                MultiBufferOffset::new(3),
            )]);
            let markers =
                layout_selected_whitespace(&selected_spaces, &layout, px(20.), window, cx)
                    .expect("选中的两个空格都应生成圆点标记");
            assert_eq!(markers.symbol.text.as_ref(), "•");
            assert_eq!(markers.origins.len(), 2);
            assert!(markers.origins[0].x < markers.origins[1].x);

            let selected_tab = SelectionSet::new(vec![crate::selection::Selection::new(
                MultiBufferOffset::new(4),
                MultiBufferOffset::new(5),
            )]);
            assert!(
                layout_selected_whitespace(&selected_tab, &layout, px(20.), window, cx,).is_none(),
                "tab 展开的空格不能被误画成多个圆点"
            );
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn caret_outside_visible_rows_is_not_painted_on_viewport_edge(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let snapshot = Buffer::from_text(
                (0..20).map(|_| "x\n").collect::<String>(),
                BufferConfig::default(),
            )
            .expect("测试 Buffer 应能创建")
            .snapshot();
            let presentation = EditorPresentation::new(&snapshot.clone().into(), None);
            let display_snapshot = project_display_snapshot(cx, snapshot.clone());
            let layout = layout_visible_lines(
                display_snapshot,
                None,
                presentation,
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(point(px(0.), px(0.)), size(px(200.), px(40.))),
                        text_clip_bounds: Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(200.), px(40.)),
                        ),
                        gutter: None,
                    },
                    active_lines: &BTreeSet::new(),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::new(10),
                    scroll_offset: point(px(0.), px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );
            let (_, carets) = layout_selections(
                &SelectionSet::caret(MultiBufferOffset::ZERO),
                &layout,
                px(20.),
                cx,
            );

            assert!(carets.is_empty());
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn multibuffer_excerpt_uses_the_same_text_selection_geometry_as_a_single_buffer(
    cx: &mut TestAppContext,
) {
    let text = "abcdef\nx\nabcde\n";
    let source_buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("应创建源 Buffer");
    let source = cx.new(move |cx| {
        LanguageBuffer::new(
            source_buffer,
            Some(PathBuf::from("src/example.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |combined, cx| {
        combined.set_excerpts_for_path(vec![ExcerptRange::line_range(source, 0..3, cx)], cx)
    });
    cx.run_until_parked();

    let multi_snapshot = cx.update_entity(&combined, |combined, cx| combined.snapshot(cx));
    let multi_text = multi_snapshot.clone();
    let single_text =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("应创建单文件 Buffer");
    let single_snapshot = single_text.snapshot();
    let selection = SelectionSet::new(vec![crate::selection::Selection::new(
        MultiBufferOffset::new(2),
        MultiBufferOffset::new(12),
    )]);

    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let geometry = EditorGeometry {
                text_bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(160.))),
                text_clip_bounds: Bounds::new(point(px(0.), px(0.)), size(px(400.), px(160.))),
                gutter: None,
            };
            let active_lines = BTreeSet::new();
            let foldable_lines = BTreeSet::new();
            let fold_anchor_lines = BTreeSet::new();
            let params = |geometry| VisibleLineLayoutParams {
                geometry,
                active_lines: &active_lines,
                foldable_lines: &foldable_lines,
                fold_anchor_lines: &fold_anchor_lines,
                start_row: DisplayRow::ZERO,
                scroll_offset: point(px(0.), px(0.)),
                primary_caret_column: None,
                line_height: px(20.),
                diff_rows: &[],
            };
            let single_layout = layout_visible_lines(
                project_display_snapshot(cx, single_snapshot.clone()),
                None,
                EditorPresentation::new(&single_snapshot.clone().into(), None),
                None,
                params(geometry),
                window,
                cx,
            );
            let multi_layout = layout_visible_lines(
                project_display_snapshot(cx, multi_snapshot),
                None,
                EditorPresentation::new(&multi_text, None),
                None,
                params(geometry),
                window,
                cx,
            );

            let (single_segments, single_carets) =
                layout_selections(&selection, &single_layout, px(20.), cx);
            let (multi_segments, multi_carets) =
                layout_selections(&selection, &multi_layout, px(20.), cx);
            let single_fragments =
                layout_background_fragments(&single_layout, &single_segments, cx);
            let multi_fragments = layout_background_fragments(&multi_layout, &multi_segments, cx);

            assert_eq!(multi_fragments.len(), single_fragments.len());
            for (multi_line, single_line) in multi_fragments.iter().zip(&single_fragments) {
                assert_eq!(
                    multi_line
                        .iter()
                        .map(|fragment| (fragment.start_x, fragment.end_x))
                        .collect::<Vec<_>>(),
                    single_line
                        .iter()
                        .map(|fragment| (fragment.start_x, fragment.end_x))
                        .collect::<Vec<_>>()
                );
            }
            // 角样式也应与单文件一致（合成管线与缓冲形态无关）。
            assert_eq!(
                multi_fragments
                    .iter()
                    .flatten()
                    .map(|fragment| fragment.corners)
                    .collect::<Vec<_>>(),
                single_fragments
                    .iter()
                    .flatten()
                    .map(|fragment| fragment.corners)
                    .collect::<Vec<_>>()
            );
            assert_eq!(multi_carets.len(), single_carets.len());
            let colors = color::current(cx);
            let selection_fragment = single_fragments
                .iter()
                .flatten()
                .find(|fragment| fragment.selection)
                .expect("应有选区片段");
            assert_eq!(
                selection_fragment.color,
                colors
                    .editor_background
                    .blend(colors.editor_selection_background)
            );
            assert_eq!(
                selection_fragment.color.a, 1.0,
                "选区颜色应在 Editor 背景上展平"
            );
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn diff_hunk_rows_maps_logical_rows_to_display_rows(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let text_system = window.text_system().clone();
            let font = window.text_style().font();
            let font_size = window.text_style().font_size.to_pixels(window.rem_size());

            // 无 wrap：逻辑行 == 显示行；纯删除空范围锚定一个显示行。
            let buffer = Buffer::from_text(
                "line 0\nline 1\nline 2\nline 3\nline 4\n".to_owned(),
                BufferConfig::default(),
            )
            .expect("应创建 Buffer");
            let snapshot = project_display_snapshot(cx, buffer.snapshot());
            assert_eq!(
                diff_hunk_rows(
                    &snapshot,
                    &[
                        DisplayHunk {
                            range: 1..2,
                            old_range: 1..2,
                            kind: DiffHunkKind::Modified,
                            staging: DiffHunkStaging::NoStaging,
                        },
                        DisplayHunk {
                            range: 3..3,
                            old_range: 2..3,
                            kind: DiffHunkKind::Deleted,
                            staging: DiffHunkStaging::NoStaging,
                        },
                        DisplayHunk {
                            range: 4..5,
                            old_range: 4..4,
                            kind: DiffHunkKind::Added,
                            staging: DiffHunkStaging::NoStaging,
                        },
                    ],
                    &[false, false, false],
                    &[],
                ),
                vec![
                    (1..2, DiffHunkKind::Modified),
                    // 折叠的删除块行内不做标记（gutter 红色胶囊提示）。
                    (4..5, DiffHunkKind::Added),
                ]
            );

            // wrap：宽行拆成多个显示行，marker 覆盖全部片段。
            let buffer = Buffer::from_text(
                "aaaa bbbb cccc dddd eeee ".repeat(10) + "\nline 1\n",
                BufferConfig::default(),
            )
            .expect("应创建 Buffer");
            let map = new_display_map(cx, buffer.snapshot());
            assert!(
                cx.update_entity(&map, |map, cx| map.set_wrap_width(
                    Some(px(100.)),
                    font.clone(),
                    font_size,
                    &text_system,
                    cx
                )),
                "宽行应产生换行"
            );
            let snapshot = cx.update_entity(&map, |map, cx| map.snapshot(cx));
            let line_count = snapshot.line_count();
            assert!(line_count > 2, "宽行应拆成多个显示行");
            let row_1 = snapshot
                .line_to_display_row(Line::new(1))
                .expect("行 1 应可映射")
                .get();

            // 行 0 wrap 成 N 段：marker 覆盖 [0, N)，N = 行 1 的行首显示行。
            assert_eq!(
                diff_hunk_rows(
                    &snapshot,
                    &[DisplayHunk {
                        range: 0..1,
                        old_range: 0..1,
                        kind: DiffHunkKind::Modified,
                        staging: DiffHunkStaging::NoStaging,
                    }],
                    &[false, false, false],
                    &[],
                ),
                vec![(0..row_1, DiffHunkKind::Modified)]
            );

            // 越界 hunk（超出文件末尾）用 line_count 收尾。
            assert_eq!(
                diff_hunk_rows(
                    &snapshot,
                    &[DisplayHunk {
                        range: 1..10,
                        old_range: 1..10,
                        kind: DiffHunkKind::Modified,
                        staging: DiffHunkStaging::NoStaging,
                    }],
                    &[false, false, false],
                    &[],
                ),
                vec![(row_1..line_count, DiffHunkKind::Modified)]
            );
        })
        .expect("测试窗口应保持可用");
}

#[gpui::test]
fn diff_hunk_rows_expanded_deleted_marks_materialized_old_rows(cx: &mut TestAppContext) {
    let collapsed = project_display_snapshot(
        cx,
        Buffer::from_text(
            "line 0\nline 1\nline 2\n".to_owned(),
            BufferConfig::default(),
        )
        .expect("应创建 Buffer")
        .snapshot(),
    );
    let collapsed_hunk = DisplayHunk {
        range: 1..1,
        old_range: 1..3,
        kind: DiffHunkKind::Deleted,
        staging: DiffHunkStaging::NoStaging,
    };
    // 未展开：行内无标记（删除点由 gutter 红色三角提示）。
    assert_eq!(
        diff_hunk_rows(
            &collapsed,
            std::slice::from_ref(&collapsed_hunk),
            &[false],
            &[None]
        ),
        vec![]
    );
    assert_eq!(
        hunk_hit_regions(
            &collapsed,
            std::slice::from_ref(&collapsed_hunk),
            &[false],
            &[None]
        ),
        vec![(1..2, 0, DiffHunkKind::Deleted)]
    );

    let expanded = project_display_snapshot(
        cx,
        Buffer::from_text(
            "line 0\nline 1\nold 1\nold 2\nline 2\n".to_owned(),
            BufferConfig::default(),
        )
        .expect("应创建 Buffer")
        .snapshot(),
    );
    let expanded_hunk = DisplayHunk {
        range: 4..4,
        old_range: 1..3,
        kind: DiffHunkKind::Deleted,
        staging: DiffHunkStaging::NoStaging,
    };
    assert_eq!(
        diff_hunk_rows(
            &expanded,
            std::slice::from_ref(&expanded_hunk),
            &[true],
            &[Some(2..4)]
        ),
        vec![(2..4, DiffHunkKind::Deleted)]
    );
    assert_eq!(
        hunk_hit_regions(&expanded, &[expanded_hunk], &[true], &[Some(2..4)]),
        vec![(2..4, 0, DiffHunkKind::Deleted)]
    );
}

#[gpui::test]
fn modified_hunk_expansion_marks_old_and_new_rows(cx: &mut TestAppContext) {
    let snapshot = project_display_snapshot(
        cx,
        Buffer::from_text(
            "line 0\nold 1\nnew 1\nline 2\n".to_owned(),
            BufferConfig::default(),
        )
        .expect("应创建 Buffer")
        .snapshot(),
    );
    let hunk = DisplayHunk {
        range: 2..3,
        old_range: 1..2,
        kind: DiffHunkKind::Modified,
        staging: DiffHunkStaging::NoStaging,
    };
    assert_eq!(
        diff_hunk_rows(
            &snapshot,
            std::slice::from_ref(&hunk),
            &[true],
            &[Some(1..2)]
        ),
        vec![(1..2, DiffHunkKind::Deleted), (2..3, DiffHunkKind::Added),]
    );
    assert_eq!(
        hunk_hit_regions(&snapshot, &[hunk], &[true], &[Some(1..2)]),
        vec![(1..3, 0, DiffHunkKind::Modified)]
    );
}

#[gpui::test]
fn modified_hunk_strip_stays_yellow_when_expanded(cx: &mut TestAppContext) {
    // 竖条色不随展开变化：展开的修改块竖条保持黄色并覆盖旧行 + 修改行。
    let snapshot = project_display_snapshot(
        cx,
        Buffer::from_text(
            "line 0\nold 1\nnew 1\nline 2\n".to_owned(),
            BufferConfig::default(),
        )
        .expect("应创建 Buffer")
        .snapshot(),
    );
    let hunk = DisplayHunk {
        range: 2..3,
        old_range: 1..2,
        kind: DiffHunkKind::Modified,
        staging: DiffHunkStaging::NoStaging,
    };
    // 展开：竖条仍黄，覆盖旧行 + 修改行（显示行 1..3）。
    assert_eq!(
        hunk_strip_rows(&snapshot, &[hunk], &[true], &[Some(1..2)]),
        vec![(1..3, DiffHunkKind::Modified)]
    );
}

/// 从显示行 chunk 流取折叠占位符的渲染描述。
fn placeholder_renderer(display: &DisplaySnapshot) -> ChunkRenderer {
    let mut renderer = None;
    let mut chunks = display.chunks(
        DisplayRow::ZERO..DisplayRow::new(display.line_count()),
        HighlightStyles::default(),
        None,
    );
    chunks.for_each_row(|event| {
        if let DisplayRowEvent::Text { chunks, .. } = event {
            for chunk in chunks {
                if let Some(current) = &chunk.renderer {
                    renderer = Some(current.clone());
                }
            }
        }
    });
    renderer.expect("折叠占位符必须携带渲染描述")
}

/// 折叠元素实测宽度回写折叠层后，软换行必须按该像素宽度切分折叠合并行；
/// 相同宽度不产生显示编辑。
#[gpui::test]
fn measured_element_width_drives_soft_wrap(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let snapshot = Buffer::from_text(
                "anchor\nhidden one\ntail".to_owned(),
                BufferConfig::default(),
            )
            .expect("测试 Buffer 应能创建")
            .snapshot();
            let map = new_display_map(cx, snapshot.clone());
            cx.update_entity(&map, |map, cx| {
                let range = {
                    let display = map.snapshot(cx);
                    let snapshot = display.buffer_snapshot();
                    snapshot.anchor_at(MultiBufferOffset::new(6), Affinity::Before)
                        ..snapshot.anchor_at(MultiBufferOffset::new(17), Affinity::After)
                };
                map.fold_range(range, FoldPlaceholder::default(), cx)
            })
            .expect("折叠应成功");

            let renderer_id =
                cx.update_entity(&map, |map, cx| placeholder_renderer(&map.snapshot(cx)).id);

            let text_style = window.text_style();
            let font_size = text_style.font_size.to_pixels(window.rem_size());
            cx.update_entity(&map, |map, cx| {
                map.set_wrap_width(
                    Some(px(120.)),
                    text_style.font(),
                    font_size,
                    window.text_system(),
                    cx,
                )
            });
            let before = cx.update_entity(&map, |map, cx| map.snapshot(cx).line_count());

            let changed = cx.update_entity(&map, |map, cx| {
                map.update_fold_widths([(renderer_id, px(400.))], cx)
            });
            assert!(changed, "宽度变化必须推进显示链");
            let after = cx.update_entity(&map, |map, cx| map.snapshot(cx).line_count());
            assert!(
                after > before,
                "实测元素宽度必须参与软换行：before={before}, after={after}"
            );

            let changed = cx.update_entity(&map, |map, cx| {
                map.update_fold_widths([(renderer_id, px(400.))], cx)
            });
            assert!(!changed, "相同实测宽度不应产生显示编辑");
        })
        .expect("测试窗口应保持可用");
}

/// 水平窗口部分覆盖折叠占位符时，chunk 仍携带渲染描述，布局仍生成元素片段。
#[gpui::test]
fn windowed_placeholder_chunk_keeps_renderer_and_element(cx: &mut TestAppContext) {
    let snapshot = Buffer::from_text(
        format!("{}\nhidden\ntail", "a".repeat(200)),
        BufferConfig::default(),
    )
    .expect("测试 Buffer 应能创建")
    .snapshot();
    let map = new_display_map(cx, snapshot.clone());
    let placeholder = FoldPlaceholder {
        collapsed_text: Some("....".into()),
        ..FoldPlaceholder::default()
    };
    cx.update_entity(&map, |map, cx| {
        let range = {
            let display = map.snapshot(cx);
            let snapshot = display.buffer_snapshot();
            snapshot.anchor_at(MultiBufferOffset::new(200), Affinity::Before)
                ..snapshot.anchor_at(MultiBufferOffset::new(207), Affinity::After)
        };
        map.fold_range(range, placeholder, cx)
    })
    .expect("折叠应成功");

    // chunk 层：窗口起点落在占位符内部（列 200..204），仍必须携带渲染描述。
    let display = cx.update_entity(&map, |map, cx| map.snapshot(cx));
    let mut clipped_has_renderer = None;
    let mut chunks = display.chunks(
        DisplayRow::ZERO..DisplayRow::new(1),
        HighlightStyles::default(),
        Some((202, 400)),
    );
    chunks.for_each_row(|event| {
        if let DisplayRowEvent::Text { chunks, .. } = event {
            for chunk in chunks {
                if chunk.is_placeholder {
                    clipped_has_renderer = Some(chunk.renderer.is_some());
                }
            }
        }
    });
    assert_eq!(
        clipped_has_renderer,
        Some(true),
        "被水平窗口裁剪的占位符 chunk 仍必须携带渲染描述"
    );

    // 布局层：同一窗口下仍必须生成占位符元素片段。
    let visual = cx.add_empty_window();
    visual.draw(
        point(px(0.), px(0.)),
        size(px(400.), px(100.)),
        |window, cx| {
            let text_style = window.text_style();
            let font_size = text_style.font_size.to_pixels(window.rem_size());
            let em_advance = window
                .text_system()
                .em_advance(
                    window.text_system().resolve_font(&text_style.font()),
                    font_size,
                )
                .expect("测试字体必须包含拉丁字形");
            let layout = layout_visible_lines(
                map.update(cx, |map, cx| map.snapshot(cx)),
                None,
                EditorPresentation::new(&snapshot.clone().into(), None),
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(point(px(0.), px(0.)), size(px(300.), px(40.))),
                        text_clip_bounds: Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(300.), px(40.)),
                        ),
                        gutter: None,
                    },
                    active_lines: &BTreeSet::new(),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::ZERO,
                    scroll_offset: point(em_advance * 266., px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );
            assert!(
                layout.lines.iter().any(|line| line
                    .line
                    .fragments
                    .iter()
                    .any(|fragment| matches!(fragment, LineFragment::Element { .. }))),
                "被窗口裁剪的占位符仍必须生成元素片段"
            );
            Empty
        },
    );
}

/// 水平滚动把整行窗口化后，选区与括号背景必须与光标走同一条
/// 「整行显示列 → 窗口内偏移」换算，不能把整行列直接作用于窗口文本。
#[gpui::test]
fn windowed_selection_geometry_matches_caret(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let snapshot =
                Buffer::from_text(format!("{}\n", "a".repeat(400)), BufferConfig::default())
                    .expect("测试 Buffer 应能创建")
                    .snapshot();
            let text_style = window.text_style();
            let font_size = text_style.font_size.to_pixels(window.rem_size());
            let em_advance = window
                .text_system()
                .em_advance(
                    window.text_system().resolve_font(&text_style.font()),
                    font_size,
                )
                .expect("测试字体必须包含拉丁字形");
            let layout = layout_visible_lines(
                project_display_snapshot(cx, snapshot.clone()),
                None,
                EditorPresentation::new(&snapshot.clone().into(), None),
                None,
                VisibleLineLayoutParams {
                    geometry: EditorGeometry {
                        text_bounds: Bounds::new(point(px(0.), px(0.)), size(px(300.), px(40.))),
                        text_clip_bounds: Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(300.), px(40.)),
                        ),
                        gutter: None,
                    },
                    active_lines: &BTreeSet::new(),
                    foldable_lines: &BTreeSet::new(),
                    fold_anchor_lines: &BTreeSet::new(),
                    start_row: DisplayRow::ZERO,
                    scroll_offset: point(em_advance * 150., px(0.)),
                    primary_caret_column: None,
                    line_height: px(20.),
                    diff_rows: &[],
                },
                window,
                cx,
            );
            assert!(
                layout.lines[0].window_start_column > 0,
                "测试必须真正进入水平窗口化"
            );

            let selections = SelectionSet::new(vec![crate::selection::Selection::new(
                MultiBufferOffset::new(150),
                MultiBufferOffset::new(200),
            )]);
            let (segments, carets) = layout_selections(&selections, &layout, px(20.), cx);
            assert_eq!(carets.len(), 1, "选区活动端必须绘制光标");
            let segment = segments[0].first().expect("选区必须生成行片段");
            let caret = carets[0].bounds.left();
            assert!(
                (segment.end_x - caret).abs() < px(1.),
                "窗口化后选区终点必须与光标 x 一致：segment={:?}, caret={caret:?}",
                segment.end_x,
            );

            let mut quads = Vec::new();
            layout_bracket_pair(
                BracketPair {
                    open: 150..151,
                    close: 151..152,
                },
                &layout,
                px(20.),
                &mut quads,
                cx,
            );
            assert_eq!(quads.len(), 2, "括号两端各生成一个背景矩形");
            let open_caret =
                layout_caret_at_buffer_offset(MultiBufferOffset::new(150), &layout, px(20.), cx)
                    .expect("括号起点光标必须可见");
            assert!(
                (quads[0].bounds.left() - open_caret.bounds.left()).abs() < px(1.),
                "窗口化后括号背景必须与光标 x 一致：quad={:?}, caret={:?}",
                quads[0].bounds.left(),
                open_caret.bounds.left(),
            );
        })
        .expect("测试窗口应保持可用");
}

/// 折叠占位符的渲染色只在 render 调用时读取：切换主题后仍能产出元素。
///
/// gpui 的 AnyElement 不暴露子元素的文字颜色，无法直接断言像素色；
/// 这里验证 render 闭包在主题切换后仍产出 Div 元素，颜色读取时机由构造签名与实现保证。
#[gpui::test]
fn ellipsis_render_reads_theme_at_call_time(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| {
            let snapshot = Buffer::from_text(
                "anchor\nhidden one\ntail".to_owned(),
                BufferConfig::default(),
            )
            .expect("测试 Buffer 应能创建")
            .snapshot();
            let map = new_display_map(cx, snapshot.clone());
            cx.update_entity(&map, |map, cx| {
                let range = {
                    let display = map.snapshot(cx);
                    let snapshot = display.buffer_snapshot();
                    snapshot.anchor_at(MultiBufferOffset::new(6), Affinity::Before)
                        ..snapshot.anchor_at(MultiBufferOffset::new(17), Affinity::After)
                };
                map.fold_range(range, FoldPlaceholder::ellipsis(), cx)
            })
            .expect("折叠应成功");
            let renderer =
                cx.update_entity(&map, |map, cx| placeholder_renderer(&map.snapshot(cx)));

            let mut dark_color = None;
            let mut light_color = None;
            for (choice, slot) in [
                (ThemeChoice::Named("dark"), &mut dark_color),
                (ThemeChoice::Named("light"), &mut light_color),
            ] {
                choice.apply(cx, Some(window));
                *slot = Some(zcv_theme::color::current(cx).text_placeholder);
                let mut element = (renderer.render)(cx);
                assert!(
                    element.downcast_mut::<gpui::Div>().is_some(),
                    "省略号占位符必须渲染成 Div"
                );
            }
            assert_ne!(dark_color, light_color, "测试主题必须提供不同的占位符颜色");
        })
        .expect("测试窗口应保持可用");
}
