use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use gpui::{Modifiers, MouseButton, TestAppContext, point, px};
use std::path::PathBuf;
use zcv_buffer_diff::{BufferDiff, BufferDiffInput, DiffHunkKind, DiffHunkStaging};
use zcv_multi_buffer::{DiffFile, DisplayHunk, ExcerptRange, MultiBuffer, ResolvedDiffHunk};
use zcv_text::{Affinity, Buffer, BufferConfig, ByteOffset, Edit, Line, TransactionMetadata};

use super::common::{
    buffer_text, focus_editor, inject_editor_diff, inject_file_diff, revision_buffer, test_buffer,
};
use super::*;
use crate::display_map::test_support::{WrapRowKind, projected_line_text};
use crate::display_map::{DisplayColumn, DisplayPoint, DisplayRow, hunk_rendering};

/// 由绝对坐标切片构造按段解析输入，供渲染单元测试调用。
fn resolved_hunks(
    hunks: Vec<DisplayHunk>,
    expanded: Vec<bool>,
    old_ranges: Vec<Option<Range<usize>>>,
    word_diffs: Vec<WordDiffs>,
) -> Vec<ResolvedDiffHunk> {
    hunks
        .into_iter()
        .enumerate()
        .map(|(index, hunk)| ResolvedDiffHunk {
            hunk,
            old_range: old_ranges.get(index).cloned().flatten(),
            expanded: expanded.get(index).copied().unwrap_or(false),
            word_diffs: word_diffs.get(index).cloned().unwrap_or_default(),
        })
        .collect()
}

/// 构造 context_lines=2 的裁剪投影项，供组合文档裁剪测试复用。
fn clipped_diff_file(
    working: Entity<LanguageBuffer>,
    base_text: &str,
    cx: &mut gpui::Context<MultiBuffer>,
) -> DiffFile {
    let path = PathBuf::from("src/a.rs");
    let base = revision_buffer(base_text, &path, cx);
    let diff = cx.new(|cx| {
        BufferDiff::new(
            BufferDiffInput {
                operations: None,
                working,
                base: Some(base),
                index: None,
                path,
            },
            cx,
        )
    });
    DiffFile {
        diff,
        display_path: PathBuf::from("src/a.rs"),
        context_lines: Some(2),
    }
}

struct OccludingHunkControls;

impl DiffHunkDelegate for OccludingHunkControls {
    fn render_hunk_controls(
        &self,
        _target: &HunkControlTarget,
        _row: usize,
        _editor: &Entity<Editor>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<AnyElement> {
        Some(
            div()
                .id("test-hunk-controls")
                .debug_selector(|| "test-hunk-controls".into())
                .occlude()
                .into_any_element(),
        )
    }
}

#[gpui::test]
fn single_file_diff_uses_the_composite_projection_path(cx: &mut TestAppContext) {
    let source = test_buffer(cx, "a\nworking\nc\n");
    let editor = cx.new(|cx| Editor::from_language_buffer(source.clone(), EditorMode::Full, cx));
    inject_file_diff(&editor, &source, Arc::from("a\nold\nc\n"), cx);

    assert_eq!(buffer_text(&source, cx), "a\nworking\nc\n");
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor
                .display_snapshot(cx)
                .buffer_snapshot()
                .excerpts()
                .count(),
            3
        );
        assert_eq!(editor.diff_hunks(cx).len(), 1);
        assert_eq!(editor.diff_hunk_old_ranges(cx), &[None]);
        assert_eq!(editor.diff_hunk_expanded(cx), vec![false]);
    });

    editor.update(cx, |editor, cx| editor.clear_diffs(cx));
    assert_eq!(buffer_text(&source, cx), "a\nworking\nc\n");
    assert!(cx.read_entity(&editor, |editor, cx| editor.diff_hunks(cx).is_empty()));
}

#[gpui::test]
fn diff_decorations_are_cached_and_consumed_by_viewport(cx: &mut TestAppContext) {
    let source = test_buffer(cx, "a\nworking\nc\n");
    let editor = cx.new(|cx| Editor::from_language_buffer(source.clone(), EditorMode::Full, cx));
    inject_file_diff(&editor, &source, Arc::from("a\nold\nc\n"), cx);

    let cached = editor.update(cx, |editor, cx| {
        editor.display_snapshot(cx).diff_decorations()
    });
    let reused = editor.update(cx, |editor, cx| {
        editor.display_snapshot(cx).diff_decorations()
    });
    assert!(
        std::sync::Arc::ptr_eq(&cached, &reused),
        "没有显示映射变化时，diff 装饰应复用同一快照"
    );

    assert!(
        cached.rendering_for_viewport(0..1).diff_rows.is_empty(),
        "视口外的 diff 行不应进入本帧消费数据"
    );
    assert_eq!(
        cached.rendering_for_viewport(1..2).diff_rows.len(),
        1,
        "视口内的 diff 行应从派生快照切片得到"
    );
}

/// 回归：同一行内编辑且 diff 几何未变化时，显示链只替换文本快照，
/// 不重建已投影的 diff 装饰。
#[gpui::test]
fn geometry_preserving_diff_edit_reuses_diff_decorations(cx: &mut TestAppContext) {
    let source = test_buffer(cx, "a\nb\nc\n");
    let editor = cx.new(|cx| Editor::from_language_buffer(source.clone(), EditorMode::Full, cx));
    inject_file_diff(&editor, &source, Arc::from("a\nB\nc\n"), cx);

    let before = editor.update(cx, |editor, cx| {
        editor.display_snapshot(cx).diff_decorations()
    });
    let before_input = editor.update(cx, |editor, cx| {
        editor
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx).diff_display().cloned())
            .expect("已注入 diff 必须有显示输入")
    });
    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                [Edit::replace(
                    MultiBufferRange::new(MultiBufferOffset::new(2), MultiBufferOffset::new(3))
                        .expect("测试范围必须有效")
                        .into(),
                    "z",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .expect("源编辑应成功");
    });
    cx.run_until_parked();

    let after = editor.update(cx, |editor, cx| {
        editor.display_snapshot(cx).diff_decorations()
    });
    let after_input = editor.update(cx, |editor, cx| {
        editor
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx).diff_display().cloned())
            .expect("已注入 diff 必须有显示输入")
    });
    assert!(
        Arc::ptr_eq(&before_input, &after_input),
        "同一行内编辑未改变 hunk 几何时不得替换 diff 显示输入"
    );
    assert!(
        Arc::ptr_eq(&before, &after),
        "同一行内编辑未改变 hunk 几何时不得重建 diff 装饰"
    );
}

#[gpui::test]
fn switching_single_file_diff_after_source_edit_keeps_text_consumer_aligned(
    cx: &mut TestAppContext,
) {
    let source = test_buffer(cx, "a\nworking\nc\n");
    let editor = cx.new(|cx| Editor::from_language_buffer(source.clone(), EditorMode::Full, cx));
    inject_file_diff(&editor, &source, Arc::from("a\nold\nc\n"), cx);

    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                [Edit::insert(MultiBufferOffset::ZERO.into(), "prefix\n").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("源编辑应成功");
    });
    cx.run_until_parked();

    editor.update(cx, |editor, cx| editor.clear_diffs(cx));
    cx.run_until_parked();

    cx.read_entity(&editor, |editor, cx| {
        let snapshot = editor.display_snapshot(cx).buffer_snapshot().clone();
        assert_eq!(
            String::from_utf8(snapshot.text_bytes()).expect("编辑器快照必须是 UTF-8"),
            "prefix\na\nworking\nc\n"
        );
        assert_eq!(editor.display_snapshot(cx).line_count(), 5);
    });
}

#[gpui::test]
fn clicking_deep_after_fold_preserves_the_visual_column(cx: &mut TestAppContext) {
    let text = include_str!("../../../../assets/keymaps/default-macos.json");
    let raw_buffer = Buffer::from_text(text.to_owned(), BufferConfig::default())
        .expect("keymap 测试 Buffer 应能创建");
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            raw_buffer,
            Some(PathBuf::from("default-macos.json")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let (editor, cx) = cx.add_window_view({
        let language_buffer = language_buffer.clone();
        move |_, cx| Editor::from_language_buffer(language_buffer, EditorMode::Full, cx)
    });
    cx.run_until_parked();

    editor.update(cx, |editor, cx| {
        editor.toggle_fold_at_line(Line::new(1), cx)
    });
    cx.refresh().expect("折叠后的编辑器应能刷新");

    let target_offset = MultiBufferOffset::new(text.find("行内").expect("测试文本应包含 行内"));
    let target_end = MultiBufferOffset::new(target_offset.get() + "行内".len());
    let (click, line_height) = cx.read_entity(&editor, |editor, _| {
        let layout = editor
            .input_layout
            .as_ref()
            .expect("刷新后应有输入命中布局");
        (
            layout
                .caret_position_for_offset(target_end)
                .expect("折叠块后的目标文本应有可见位置"),
            layout.line_height(),
        )
    });
    cx.simulate_click(
        point(click.x + px(1.), click.y + line_height * 0.5),
        gpui::Modifiers::default(),
    );

    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().head(),
            target_end,
            "折叠块后的点击必须保持视觉列，不能把 行内 命中到 局部 后面"
        );
    });
}

#[gpui::test]
fn hunk_controls_remain_visible_when_pointer_enters_controls(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "line0\nline1\nline2\n");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx)
    });
    let source = buffer.clone();
    editor.update(cx, |editor, cx| {
        editor.set_diff_hunk_delegate(Some(Arc::new(OccludingHunkControls)), cx);
    });
    inject_file_diff(&editor, &source, Arc::from("line0\nold\nline2\n"), cx);
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");

    let (window_bounds, line_height) =
        cx.update(|window, _| (window.bounds(), window.line_height()));
    let hunk_point = point(
        window_bounds.right() - px(120.),
        window_bounds.top() + line_height * 1.5,
    );
    cx.simulate_mouse_move(hunk_point, None, Modifiers::default());
    cx.refresh().expect("进入 hunk 后应刷新");
    let controls = cx
        .debug_bounds("test-hunk-controls")
        .expect("悬停 hunk 时应显示操作栏");

    let controls_center = point(
        controls.left() + controls.size.width * 0.5,
        controls.top() + controls.size.height * 0.5,
    );
    cx.simulate_mouse_move(controls_center, None, Modifiers::default());
    cx.refresh().expect("进入操作栏后应刷新");
    assert!(
        cx.debug_bounds("test-hunk-controls").is_some(),
        "操作栏自身的命中区域不能让 hunk 悬停状态失效"
    );
}

#[gpui::test]
fn hunk_controls_stick_to_viewport_while_hunk_start_is_scrolled_out(cx: &mut TestAppContext) {
    let text = (0..80)
        .map(|line| format!("line{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let buffer = test_buffer(cx, text);
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx)
    });
    let source = buffer.clone();
    editor.update(cx, |editor, cx| {
        editor.set_diff_hunk_delegate(Some(Arc::new(OccludingHunkControls)), cx);
    });
    inject_editor_diff(
        &editor,
        &source,
        vec![DisplayHunk {
            range: 1..50,
            old_range: 1..1,
            kind: DiffHunkKind::Added,
            staging: DiffHunkStaging::NoStaging,
        }],
        None,
        cx,
    );
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");

    let (window_bounds, line_height) =
        cx.update(|window, _| (window.bounds(), window.line_height()));
    cx.update_entity(&editor, |editor, cx| {
        assert!(editor.scroll_to(line_height * 10., cx));
    });
    cx.run_until_parked();
    cx.refresh().expect("滚动后测试窗口应可刷新");

    let visible_hunk_point = point(
        window_bounds.right() - px(120.),
        window_bounds.top() + line_height * 2.5,
    );
    cx.simulate_mouse_move(visible_hunk_point, None, Modifiers::default());
    cx.refresh().expect("悬停可见 hunk 后应刷新");

    let controls = cx
        .debug_bounds("test-hunk-controls")
        .expect("hunk 起点滚出视口后，操作栏仍应显示");
    let top_offset = (controls.top() - window_bounds.top()).abs() / px(1.);
    assert!(
        top_offset <= 1.,
        "操作栏应吸附在可见区顶部，实际偏移 {top_offset}px"
    );
}

#[gpui::test]
fn deleted_hunk_expands_and_collapses_readonly_excerpt(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\nb\nc");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx)
    });
    let source = buffer.clone();
    cx.run_until_parked();
    let base_rows = cx.read_entity(&editor, |editor, cx| {
        editor.display_snapshot(cx).line_count()
    });
    assert_eq!(base_rows, 3);

    // 注入 Deleted hunk（新侧行 1 处删除了 HEAD 的 1..3 行）+ HEAD 全文。
    inject_editor_diff(
        &editor,
        &source,
        vec![DisplayHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
            staging: DiffHunkStaging::NoStaging,
        }],
        Some(Arc::from("a\nold1\nold2\nb\nc")),
        cx,
    );
    // 未展开：行数不变。
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .line_count()),
        3
    );
    assert!(
        cx.read_entity(&editor, |editor, cx| {
            !editor
                .diff_hunk_expanded(cx)
                .iter()
                .any(|&expanded| expanded)
        }),
        "普通编辑器的 hunk 应默认折叠"
    );

    // 展开删除块：HEAD 的 1..3 行（old1/old2）作为只读 excerpt 插入。
    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .line_count()),
        5,
        "展开后应增加 2 个被删除行"
    );

    // 点击展开块的 gutter 色带折叠：回到 3 行。
    cx.refresh().expect("展开删除块后应能刷新");
    let (window_bounds, line_height) =
        cx.update(|window, _| (window.bounds(), window.line_height()));
    cx.simulate_mouse_down(
        point(
            window_bounds.left() + px(1.),
            window_bounds.top() + line_height * 1.5,
        ),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .line_count()),
        3,
        "点击 gutter 折叠后应回到 3 行"
    );
}

#[gpui::test]
fn added_hunk_strip_expands_from_any_row(cx: &mut TestAppContext) {
    // 普通文档中的纯新增块：多行色条的每一行都应可点击展开 / 折叠。
    let buffer = test_buffer(cx, "a\nx1\nx2\nx3\nb");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx)
    });
    let source = buffer.clone();
    cx.run_until_parked();
    inject_editor_diff(&editor, &source, Vec::new(), Some(Arc::from("a\nb")), cx);
    cx.run_until_parked();

    let (window_bounds, line_height) =
        cx.update(|window, _| (window.bounds(), window.line_height()));
    let strip_x = window_bounds.left() + px(1.);
    let row_y = |row: f32| window_bounds.top() + line_height * (row + 0.5);
    let expanded = |cx: &mut TestAppContext| {
        cx.read_entity(&editor, |editor, cx| {
            editor
                .diff_hunk_expanded(cx)
                .first()
                .copied()
                .unwrap_or(false)
        })
    };

    // 首行展开，再点击首行折叠。
    cx.simulate_mouse_down(
        point(strip_x, row_y(1.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert!(expanded(cx), "点击新增块首行应展开");
    cx.simulate_mouse_down(
        point(strip_x, row_y(1.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert!(!expanded(cx), "再次点击首行应折叠");

    // 点击末行同样应展开。
    cx.simulate_mouse_down(
        point(strip_x, row_y(3.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert!(expanded(cx), "点击新增块末行也应展开");
}

#[gpui::test]
fn added_hunk_strip_clickable_when_start_scrolled_out(cx: &mut TestAppContext) {
    // 起点滚出视口后，可见部分仍应有 hitbox（与绘制侧 visible_block_extent 一致）。
    let text = (0..80)
        .map(|line| format!("line{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let buffer = test_buffer(cx, text);
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx)
    });
    let source = buffer.clone();
    inject_editor_diff(&editor, &source, Vec::new(), None, cx);
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");

    let (window_bounds, line_height) =
        cx.update(|window, _| (window.bounds(), window.line_height()));
    cx.update_entity(&editor, |editor, cx| {
        assert!(editor.scroll_to(line_height * 10., cx));
    });
    cx.run_until_parked();
    cx.refresh().expect("滚动后测试窗口应可刷新");

    cx.simulate_mouse_down(
        point(
            window_bounds.left() + px(1.),
            window_bounds.top() + line_height * 2.5,
        ),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert!(
        cx.read_entity(&editor, |editor, cx| editor
            .diff_hunk_expanded(cx)
            .iter()
            .any(|&expanded| expanded)),
        "起点滚出视口后，可见色带仍应可点击展开"
    );
}

#[gpui::test]
fn toggle_fold_collapses_and_expands_the_cursor_block(cx: &mut TestAppContext) {
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("main.rs")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    cx.run_until_parked();
    // 语法候选不缓存进 CreaseMap，而是在显示层请求入口行时即时生成。
    assert!(cx.read_entity(&editor, |editor, cx| {
        editor
            .display_snapshot(cx)
            .crease_at_line(Line::ZERO)
            .is_some()
    }));
    assert!(cx.read_entity(&editor, |editor, cx| {
        editor
            .display_snapshot(cx)
            .crease_at_line(Line::new(3))
            .is_some()
    }));

    // 折叠 fn main（入口行 0）：隐藏块内 2 行，无占位行，总行数 6 → 4。
    editor.update(cx, |editor, cx| editor.toggle_fold_at_line(Line::ZERO, cx));
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .line_count()),
        4
    );
    assert!(cx.read_entity(&editor, |editor, cx| {
        editor
            .display_snapshot(cx)
            .fold_anchor_lines()
            .contains(&Line::ZERO)
    }));

    // 再次切换：展开，恢复 6 行。
    editor.update(cx, |editor, cx| editor.toggle_fold_at_line(Line::ZERO, cx));
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .line_count()),
        6
    );
    assert!(!cx.read_entity(&editor, |editor, cx| {
        editor
            .display_snapshot(cx)
            .fold_anchor_lines()
            .contains(&Line::ZERO)
    }));
}

#[gpui::test]
fn explicit_crease_is_visible_and_removable_by_identity(cx: &mut TestAppContext) {
    let buffer = Buffer::from_text("heading\nbody\n".to_owned(), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("notes.txt")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx));

    let ids = editor.update(cx, |editor, cx| {
        let snapshot = editor.display_snapshot(cx).buffer_snapshot().clone();
        let range = snapshot.anchor_at(MultiBufferOffset::new(0), Affinity::Before)
            ..snapshot.anchor_at(snapshot.len_bytes(), Affinity::After);
        editor.insert_creases([range], cx)
    });
    assert_eq!(ids.len(), 1, "每个显式范围应获得一个稳定身份");
    assert!(cx.read_entity(&editor, |editor, cx| {
        editor
            .display_snapshot(cx)
            .crease_at_line(Line::ZERO)
            .is_some()
    }));

    editor.update(cx, |editor, cx| editor.remove_creases(ids, cx));
    assert!(cx.read_entity(&editor, |editor, cx| {
        editor
            .display_snapshot(cx)
            .crease_at_line(Line::ZERO)
            .is_none()
    }));
}

#[gpui::test]
fn toggle_fold_action_uses_the_cursor_block_and_the_whole_folded_row(cx: &mut TestAppContext) {
    let text = "fn main() {\n    if true {\n        let x = 1;\n    }\n}\nfn other() {}";
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("main.rs")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(language_buffer, EditorMode::Full, cx));
    cx.run_until_parked();

    // 光标在 if 块内部时，折叠包含它的最内层范围，而不要求位于 crease 所在行。
    editor.update(cx, |editor, cx| {
        editor.set_selections(
            SelectionSet::caret(MultiBufferOffset::new(
                text.find("let x").expect("测试文本应包含 let x"),
            )),
            cx,
        );
        editor.toggle_fold_at_cursor(cx);
    });
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .line_count()),
        4,
        "应只折叠内层 if 块"
    );

    // 光标位于折叠占位符之后的闭合尾段时，仍按同一显示行展开。
    editor.update(cx, |editor, cx| {
        editor.set_selections(
            SelectionSet::caret(MultiBufferOffset::new(
                text.find("    }\n}").expect("测试文本应包含内层闭合括号") + 4,
            )),
            cx,
        );
        editor.toggle_fold_at_cursor(cx);
    });
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .line_count()),
        6,
        "折叠合并行任意位置都应能展开"
    );
}

#[gpui::test]
fn clicking_the_crease_toggles_fold_without_selecting_the_line(cx: &mut TestAppContext) {
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("main.rs")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let (editor, cx) = cx.add_window_view({
        let language_buffer = language_buffer.clone();
        move |_, cx| Editor::from_language_buffer(language_buffer, EditorMode::Full, cx)
    });
    cx.run_until_parked();

    let initial_selection = SelectionSet::caret(MultiBufferOffset::new(3));
    editor.update(cx, |editor, cx| {
        editor.set_selections(initial_selection.clone(), cx);
    });

    // 默认测试字体下，首行 crease 位于 gutter 右侧的折叠指示列中心。
    cx.simulate_click(point(px(54.), px(12.)), gpui::Modifiers::default());

    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.display_snapshot(cx).line_count(),
            4,
            "点击 crease 应折叠首个函数"
        );
        assert_eq!(
            editor.selections(cx),
            initial_selection,
            "crease 点击不应继续冒泡成 gutter 整行选择"
        );
    });
}

#[gpui::test]
fn expanding_diff_hunk_preserves_code_fold(cx: &mut TestAppContext) {
    // 展开 diff hunk 只是重排组合文本，不得把已折叠的代码展开。
    let working = "fn main() {\n    let a = 1;\n    let b = 2;\n}\nfn other() {\n    let x = 1;\n    let y = 2;\n}\n";
    let base = "fn main() {\n    let a = 1;\n    let b = 99;\n}\nfn other() {\n    let x = 1;\n    let y = 2;\n}\n";
    let buffer = test_buffer(cx, working);
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/main.rs"), cx)
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx)
    });
    let source = buffer.clone();
    inject_editor_diff(&editor, &source, Vec::new(), Some(Arc::from(base)), cx);
    cx.run_until_parked();

    editor.update(cx, |editor, cx| {
        editor.toggle_fold_at_line(Line::new(4), cx)
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .fold_anchor_lines()),
        vec![Line::new(4)]
    );

    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .fold_anchor_lines()),
        vec![Line::new(5)],
        "展开 hunk 后已折叠代码应保持折叠，锚点随插入的旧侧行下移"
    );
}

#[gpui::test]
fn expanding_diff_hunk_keeps_crease_of_enclosing_fold(cx: &mut TestAppContext) {
    // 折叠范围包含 diff hunk 时，展开 hunk 会把工作区 excerpt 切开，
    // crease 用的 fold_ranges 仍必须跨连续的同类 excerpt 投影出来。
    let working = "fn main() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n}\n";
    let base = "fn main() {\n    let a = 1;\n    let b = 99;\n    let c = 3;\n}\n";
    let buffer = test_buffer(cx, working);
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/main.rs"), cx)
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx)
    });
    let source = buffer.clone();
    inject_editor_diff(&editor, &source, Vec::new(), Some(Arc::from(base)), cx);
    cx.run_until_parked();
    assert!(
        cx.read_entity(&editor, |editor, cx| {
            editor
                .display_snapshot(cx)
                .crease_at_line(Line::ZERO)
                .is_some()
        }),
        "fn main 展开前应有折叠范围"
    );

    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    cx.run_until_parked();
    assert!(
        cx.read_entity(&editor, |editor, cx| {
            editor
                .display_snapshot(cx)
                .crease_at_line(Line::ZERO)
                .is_some()
        }),
        "展开 hunk 后，包含 hunk 的折叠按钮必须保留"
    );
}

#[gpui::test]
fn fold_ranges_survive_edits_and_folded_state_follows(cx: &mut TestAppContext) {
    // 回归：编辑后折叠范围与折叠状态必须保持（crease 箭头显示依赖 fold_ranges / fold_anchor_lines）。
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("main.rs")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let editor =
        cx.new(|cx| Editor::from_language_buffer(language_buffer.clone(), EditorMode::Full, cx));
    cx.run_until_parked();

    // 编辑 buffer：在首行后插入一行注释。
    cx.update_entity(&language_buffer, |language_buffer, cx| {
        language_buffer
            .edit(
                [Edit::insert(MultiBufferOffset::new(7).into(), "// 注释\n").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("插入应成功");
    });
    cx.run_until_parked();

    // 编辑后按行查询的语言折叠仍可用（插值树版本与 buffer 同步）。
    assert!(
        cx.read_entity(&editor, |editor, cx| {
            editor
                .display_snapshot(cx)
                .crease_at_line(Line::new(1))
                .is_some()
        }),
        "编辑后首个函数折叠应保持可用"
    );
    assert!(
        cx.read_entity(&editor, |editor, cx| {
            editor
                .display_snapshot(cx)
                .crease_at_line(Line::new(4))
                .is_some()
        }),
        "编辑后第二个函数折叠应保持可用"
    );

    // 注释行插入后 `{` 落到行 1（fold 范围起点行随编辑推进），入口行折叠仍可用。
    editor.update(cx, |editor, cx| {
        editor.toggle_fold_at_line(Line::new(1), cx)
    });
    assert!(cx.read_entity(&editor, |editor, cx| {
        editor
            .display_snapshot(cx)
            .fold_anchor_lines()
            .contains(&Line::new(1))
    }));
}
#[gpui::test]
fn folded_bracket_highlight_lands_on_merged_row(cx: &mut TestAppContext) {
    // 回归：折叠块后光标在入口行 `{` 上，另一半括号高亮投影到合并行的真实 `}` 列。
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("main.rs")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    cx.run_until_parked();
    editor.update(cx, |editor, cx| editor.toggle_fold_at_line(Line::ZERO, cx));

    // 光标在 `{` 上（字节 10；字节 8/9 会命中 `()` 对）。
    let close_range = cx.update_entity(&editor, |editor, cx| {
        editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(10)), cx);
        let pair = editor
            .matching_bracket_pair(cx)
            .expect("光标旁的括号应由 tree-sitter query 匹配");
        pair.close.clone()
    });
    // 合并行文本：anchor + 占位符 + 真实 `}`。
    let snapshot = cx.read_entity(&editor, |editor, cx| editor.display_snapshot(cx));
    let mut cursor = snapshot.rows(DisplayRow::ZERO, 1);
    let row = cursor.next().expect("视口应可读取");
    let WrapRowKind::Text { projected_line, .. } = row.kind();
    assert_eq!(
        projected_line_text(&snapshot, *projected_line)
            .unwrap()
            .as_ref(),
        "fn main() {⋯}\n"
    );
    // 真实 `}` 范围投影到合并行占位符之后的列（anchor 11 字符 + 占位符 1 列 = 12）。
    let projected = snapshot
        .project_text_range(
            MultiBufferRange::new(
                MultiBufferOffset::new(close_range.start),
                MultiBufferOffset::new(close_range.end),
            )
            .expect("`}` 范围应合法"),
        )
        .expect("投影应成功");
    assert_eq!(projected.len(), 1);
    assert_eq!(
        projected[0].start(),
        DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(12))
    );
    assert_eq!(
        projected[0].end(),
        DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(13))
    );
}

#[gpui::test]
fn horizontal_movement_jumps_over_folded_content(cx: &mut TestAppContext) {
    // 折叠在显示上占一个字符：右箭头从折叠起点一步跨到闭合括号，左箭头回到折叠起点。
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("main.rs")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let (editor, cx) = cx.add_window_view({
        let language_buffer = language_buffer.clone();
        move |_, cx| Editor::from_language_buffer(language_buffer, EditorMode::Full, cx)
    });
    cx.run_until_parked();
    editor.update(cx, |editor, cx| editor.toggle_fold_at_line(Line::ZERO, cx));

    // 光标在折叠起点（anchor 行行尾，字节 11）。
    editor.update(cx, |editor, cx| {
        editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(11)), cx);
    });
    focus_editor(&editor, cx);
    cx.dispatch_action(MoveRight);
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().head(),
            MultiBufferOffset::new(27),
            "右箭头应一步跨过折叠，落在闭合括号"
        );
    });
    cx.dispatch_action(MoveLeft);
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().head(),
            MultiBufferOffset::new(11),
            "左箭头应回到折叠起点"
        );
    });

    // 选区扩展也把折叠视为一个显示单元；跨过占位符后，下一次扩展必须继续进入可见尾段。
    editor.update(cx, |editor, cx| {
        editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(11)), cx);
    });
    cx.dispatch_action(SelectRight);
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary(),
            &Selection::new(MultiBufferOffset::new(11), MultiBufferOffset::new(27)),
            "第一次向右扩展应一次选中折叠源范围"
        );
    });
    cx.dispatch_action(SelectRight);
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary(),
            &Selection::new(MultiBufferOffset::new(11), MultiBufferOffset::new(28)),
            "选中折叠后仍应能继续向右扩展"
        );
    });
}

#[gpui::test]
fn folded_rows_keep_the_following_line_clickable_and_editable(cx: &mut TestAppContext) {
    let text = "before\nfn folded() {\n  let value = 1;\n}\nafter\n";
    let raw_buffer = Buffer::from_text(text.to_owned(), BufferConfig::default())
        .expect("Rust 测试 Buffer 应能创建");
    let buffer = cx.new(|cx| {
        LanguageBuffer::new(
            raw_buffer,
            Some(PathBuf::from("main.rs")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx)
    });
    cx.run_until_parked();

    // 折叠第 1 行的对象，显示上应只保留合并行、前后可见行和末尾行。
    editor.update(cx, |editor, cx| {
        editor.toggle_fold_at_line(Line::new(1), cx)
    });
    let after_offset = MultiBufferOffset::new(text.find("after").expect("测试文本应包含 after"));

    focus_editor(&editor, cx);
    editor.update(cx, |editor, cx| {
        editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(7)), cx);
    });
    cx.dispatch_action(MoveDown);
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().head(),
            after_offset,
            "折叠后的下一行应能通过向下移动到达；显示行数={}，光标位置={:?}",
            editor.display_snapshot(cx).line_count(),
            editor
                .render_snapshot(cx)
                .byte_to_position(editor.selections(cx).primary().head())
        );
    });

    cx.refresh().expect("折叠后的编辑器应能刷新");
    let line_height = cx.read_entity(&editor, |editor, _| {
        editor.last_line_height.expect("渲染后应有行高")
    });
    cx.simulate_click(
        point(px(100.), px(2.) + line_height * 2.),
        gpui::Modifiers::default(),
    );
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor
                .render_snapshot(cx)
                .byte_to_position(editor.selections(cx).primary().head())
                .expect("点击后的光标应有效")
                .line(),
            Line::new(4),
            "折叠后的下一行应能通过点击获得光标"
        );
    });

    cx.simulate_input("!");
    assert_eq!(buffer_text(&buffer, cx), text.replace("after", "aft!er"));
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.display_snapshot(cx).line_count(),
            4,
            "编辑后折叠应保持；折叠入口={:?}",
            editor.display_snapshot(cx).fold_anchor_lines(),
        );
    });
}

#[gpui::test]
fn unfold_all_expands_every_fold(cx: &mut TestAppContext) {
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("main.rs")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    cx.run_until_parked();

    // 手动折叠两个块体（各自隐藏 2 行，无占位行）：总行数 6 → 2。
    editor.update(cx, |editor, cx| editor.toggle_fold_at_line(Line::ZERO, cx));
    editor.update(cx, |editor, cx| {
        editor.toggle_fold_at_line(Line::new(3), cx)
    });
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .line_count()),
        2
    );

    // 全部展开：恢复 6 行。
    editor.update(cx, |editor, cx| editor.unfold_all_ranges(cx));
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .line_count()),
        6
    );
    assert!(!cx.read_entity(&editor, |editor, cx| {
        editor
            .display_snapshot(cx)
            .fold_anchor_lines()
            .contains(&Line::ZERO)
    }));
}
#[gpui::test]
fn diff_hunks_follow_buffer_edits_without_losing_highlight(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "line 0\nCHANGED\nline 2\n");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let editor = cx.new(|cx| Editor::for_language_buffer(buffer.clone(), cx));
    let source = buffer.clone();

    inject_editor_diff(
        &editor,
        &source,
        Vec::new(),
        Some(Arc::from("line 0\nline 1\nline 2\n")),
        cx,
    );
    editor.update(cx, |editor, cx| {
        assert_eq!(
            editor.diff_hunks(cx),
            &[DisplayHunk {
                range: 1..2,
                old_range: 1..2,
                kind: DiffHunkKind::Modified,
                staging: DiffHunkStaging::NoStaging,
            }],
            "注入后应立即可见"
        );

        // 在 hunk 前插入一行后版本推进，已有修改应跟随文本移动到新行而不是消失。
        editor.multi_buffer.update(cx, |buffer, cx| {
            buffer
                .edit(
                    vec![Edit::insert(MultiBufferOffset::ZERO.into(), "changed\n").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .expect("测试编辑应成功");
        });
    });
    // hunk 重算由 BufferDiff 自行完成并异步发出事件：等一轮 effect 后再断言。
    cx.run_until_parked();
    assert!(
        cx.read_entity(&editor, |editor, cx| editor.diff_hunks(cx).iter().any(
            |hunk| hunk
                == &DisplayHunk {
                    range: 2..3,
                    old_range: 1..2,
                    kind: DiffHunkKind::Modified,
                    staging: DiffHunkStaging::NoStaging,
                }
        )),
        "编辑后 Git 高亮应跟随文本位置"
    );
    // 后续 Git 刷新仍可用新的权威结果替换当前投影（base 为空 → 整份新增）。
    inject_editor_diff(&editor, &source, Vec::new(), None, cx);
    editor.update(cx, |editor, cx| {
        assert_eq!(editor.diff_hunks(cx).len(), 1, "重新注入后应恢复");
    });
}

#[gpui::test]
fn external_reparse_refreshes_added_diff_syntax_highlights(cx: &mut TestAppContext) {
    let source = test_buffer(cx, "fn main() {\n    let value = 1;\n}\n");
    source.update(cx, |source, cx| {
        source.set_file_path(PathBuf::from("src/main.rs"), cx);
    });
    let editor = cx.new(|cx| Editor::for_language_buffer(source.clone(), cx));
    inject_file_diff(
        &editor,
        &source,
        Arc::from("fn main() {\n    let value = 0;\n}\n"),
        cx,
    );
    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));

    cx.update_entity(&source, |source, cx| {
        let old_line = "fn main() {\n    let value = 1;\n";
        source
            .edit(
                [Edit::replace(
                    MultiBufferRange::new(
                        MultiBufferOffset::new("fn main() {\n".len()),
                        MultiBufferOffset::new(old_line.len()),
                    )
                    .unwrap()
                    .into(),
                    "    // 外部编辑\n    let value = 1;\n",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .expect("外部源编辑应成功");
    });
    cx.run_until_parked();

    cx.read_entity(&editor, |editor, cx| {
        let snapshot = editor.display_snapshot(cx);
        let hunk = &editor.diff_hunks(cx)[0];
        let buffer = snapshot.buffer_snapshot();
        let start = buffer
            .line_start_byte(Line::new(hunk.range.start))
            .expect("新增 hunk 起点应位于组合文档中")
            .get();
        let end = if hunk.range.end < buffer.line_count() {
            buffer
                .line_start_byte(Line::new(hunk.range.end))
                .expect("新增 hunk 终点应位于组合文档中")
                .get()
        } else {
            buffer.len_bytes().get()
        };
        let names = buffer.capture_names();
        assert!(
            buffer.highlights(start..end).iter().any(|span| {
                names
                    .get(span.capture as usize)
                    .is_some_and(|name| name.as_ref() == "comment")
            }),
            "外部重解析后，绿色 working excerpt 应保留新语法高亮"
        );
    });
}

#[gpui::test]
fn soft_wrap_renders_continuation_rows_and_click_hits_fragment(cx: &mut TestAppContext) {
    // 超长行（超出测试窗口宽度）在 editor-width 模式下拆成多个显示行。
    let buffer = test_buffer(cx, "    aaaa bbbb cccc dddd eeee ".repeat(10));
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    editor.update(cx, |editor, cx| {
        editor.set_soft_wrap_mode(Some(SoftWrap::EditorWidth), cx);
    });
    cx.run_until_parked();

    let (line_count, continuation_offset) = cx.read_entity(&editor, |editor, cx| {
        let line_count = editor.display_snapshot(cx).line_count();
        assert!(line_count > 1, "宽行应拆成多个显示行");
        let continuation = editor
            .display_snapshot(cx)
            .display_point_to_offset(DisplayPoint::new(DisplayRow::new(1), DisplayColumn::ZERO))
            .expect("续行行首应可映射");
        (line_count, continuation)
    });

    // 点击第二个显示行（行高约 26px），光标应落在续行片段起点。
    // x 越过 gutter（约 60px）进入文本区，落在第二个显示行内。
    cx.simulate_click(point(px(80.), px(30.)), gpui::Modifiers::default());
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().head(),
            continuation_offset,
            "点击续行应把光标放到片段起点"
        );
    });
    assert!(line_count > 0);
}

/// 单行输入宿主：把编辑器约束到窄于文本的固定宽度，模拟项目树行内名称输入框。
struct NarrowSingleLineHost {
    editor: Entity<Editor>,
}

impl Render for NarrowSingleLineHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // 80px：NoopTextSystem 等宽度量下也足以让测试文本溢出，触发水平跟随。
        div().w(px(80.)).h(px(24.)).child(self.editor.clone())
    }
}

#[gpui::test]
fn single_line_editor_never_wraps_and_follows_caret_horizontally(cx: &mut TestAppContext) {
    let editor = cx.new(Editor::single_line);
    let (_host, cx) = cx.add_window_view({
        let editor = editor.clone();
        |_, _| NarrowSingleLineHost { editor }
    });
    // 真实环境全局默认为 editor-width；单行输入必须免疫，否则长名称被切到可见范围外。
    editor.update(cx, |editor, cx| {
        editor.set_soft_wrap_mode(Some(SoftWrap::EditorWidth), cx);
        editor.set_text("抗突发错误光子太赫兹通信.md", cx);
    });
    cx.run_until_parked();

    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.display_snapshot(cx).line_count(),
            1,
            "单行输入不应拆成多个显示行"
        );
        assert!(
            editor.scroll_offset().x > Pixels::ZERO,
            "超宽文本应水平滚动让光标可见，而不是换行隐藏前段"
        );
    });
}

#[gpui::test]
fn multibuffer_soft_wrap_uses_the_regular_display_map_pipeline(cx: &mut TestAppContext) {
    let source = test_buffer(
        cx,
        "    引擎内容很长，需要在多文件编辑器中正常软换行。".repeat(30),
    );
    source.update(cx, |source, cx| {
        source.set_file_path(PathBuf::from("文档/引擎.md"), cx)
    });
    let source_end = cx.read_entity(&source, |source, _| source.len_bytes());
    let source_buffer_id = cx.read_entity(&source, |source, _| source.buffer_id());
    let source_multi = source.clone();
    let combined = cx.new(MultiBuffer::empty);
    combined.update(cx, |combined, cx| {
        combined.set_excerpts_for_path(
            vec![ExcerptRange::new(
                source_multi,
                MultiBufferRange::new(MultiBufferOffset::ZERO, source_end)
                    .expect("完整片段范围应有效")
                    .into(),
                Vec::new(),
            )],
            cx,
        );
    });

    let (editor, cx) = cx.add_window_view({
        let combined = combined.clone();
        move |_, cx| Editor::for_multi_buffer(combined, cx)
    });
    cx.run_until_parked();
    let unwrapped_rows = cx.read_entity(&editor, |editor, cx| {
        editor.display_snapshot(cx).line_count()
    });

    editor.update(cx, |editor, cx| {
        editor.set_soft_wrap_mode(Some(SoftWrap::EditorWidth), cx);
    });
    cx.run_until_parked();
    let wrapped_rows = cx.read_entity(&editor, |editor, cx| {
        editor.display_snapshot(cx).line_count()
    });

    assert!(
        wrapped_rows > unwrapped_rows,
        "MultiBuffer 应经过与普通 Editor 相同的 WrapMap；{unwrapped_rows} -> {wrapped_rows}"
    );

    editor.update(cx, |editor, cx| {
        editor.toggle_buffer_fold(source_buffer_id, cx)
    });
    let folded_rows = cx.read_entity(&editor, |editor, cx| {
        editor.display_snapshot(cx).line_count()
    });
    assert_eq!(folded_rows, 2, "整文件折叠后只保留两行高的 BufferHeader");

    editor.update(cx, |editor, cx| {
        editor.toggle_buffer_fold(source_buffer_id, cx)
    });
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .line_count()),
        wrapped_rows,
        "再次点击 header chevron 应完整恢复 excerpts"
    );
}

#[gpui::test]
fn wrapped_multibuffer_reuses_block_rows_across_within_line_edits(cx: &mut TestAppContext) {
    let first = test_buffer(cx, "let alpha = 1;\nlet beta = 2;\n");
    first.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("a.rs"), cx)
    });
    let second = test_buffer(cx, "let gamma = 3;\n");
    second.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("b.rs"), cx)
    });

    let combined = cx.new(MultiBuffer::empty);
    let first_len = cx.read_entity(&first, |buffer, _| buffer.len_bytes());
    let second_len = cx.read_entity(&second, |buffer, _| buffer.len_bytes());
    combined.update(cx, |combined, cx| {
        combined.set_excerpts_for_path(
            vec![ExcerptRange::new(
                first.clone(),
                MultiBufferRange::new(MultiBufferOffset::ZERO, first_len)
                    .expect("完整片段范围应有效")
                    .into(),
                Vec::new(),
            )],
            cx,
        );
        combined.set_excerpts_for_path(
            vec![ExcerptRange::new(
                second.clone(),
                MultiBufferRange::new(MultiBufferOffset::ZERO, second_len)
                    .expect("完整片段范围应有效")
                    .into(),
                Vec::new(),
            )],
            cx,
        );
    });

    let (editor, cx) = cx.add_window_view({
        let combined = combined.clone();
        move |_, cx| Editor::for_multi_buffer(combined, cx)
    });
    cx.run_until_parked();
    editor.update(cx, |editor, cx| {
        editor.set_soft_wrap_mode(Some(SoftWrap::EditorWidth), cx);
    });
    cx.run_until_parked();

    let before = cx.read_entity(&editor, |editor, cx| {
        let snapshot = editor.display_snapshot(cx);
        let mut rows = snapshot.rows(DisplayRow::ZERO, snapshot.line_count());
        let mut result = Vec::new();
        while let Some(row) = rows.next() {
            if let Some(block) = row.block() {
                result.push((row.index().get(), block.excerpt.path().to_path_buf()));
            }
        }
        result
    });
    assert_eq!(before.len(), 2, "两个文件各有一个 BufferHeader 块");

    // 同宽行内替换：断行结果不变，块布局应保持原样，只刷新片段视图。
    cx.update_entity(&first, |source, cx| {
        source
            .edit(
                [Edit::replace(
                    MultiBufferRange::new(MultiBufferOffset::new(4), MultiBufferOffset::new(9))
                        .expect("替换范围应有效")
                        .into(),
                    "ALPHA",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .expect("源编辑应成功");
    });
    cx.run_until_parked();

    let after = cx.read_entity(&editor, |editor, cx| {
        let snapshot = editor.display_snapshot(cx);
        let mut rows = snapshot.rows(DisplayRow::ZERO, snapshot.line_count());
        let mut result = Vec::new();
        while let Some(row) = rows.next() {
            if let Some(block) = row.block() {
                result.push((row.index().get(), block.excerpt.path().to_path_buf()));
            }
        }
        result
    });
    assert_eq!(after, before);
}

#[gpui::test]
fn wrapped_multibuffer_relocates_blocks_when_wrap_rows_change(cx: &mut TestAppContext) {
    let first = test_buffer(cx, "let alpha = 1;\nlet beta = 2;\n");
    first.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("a.rs"), cx)
    });
    let second = test_buffer(cx, "let gamma = 3;\n");
    second.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("b.rs"), cx)
    });

    let combined = cx.new(MultiBuffer::empty);
    let first_len = cx.read_entity(&first, |buffer, _| buffer.len_bytes());
    let second_len = cx.read_entity(&second, |buffer, _| buffer.len_bytes());
    combined.update(cx, |combined, cx| {
        combined.set_excerpts_for_path(
            vec![ExcerptRange::new(
                first.clone(),
                MultiBufferRange::new(MultiBufferOffset::ZERO, first_len)
                    .expect("完整片段范围应有效")
                    .into(),
                Vec::new(),
            )],
            cx,
        );
        combined.set_excerpts_for_path(
            vec![ExcerptRange::new(
                second.clone(),
                MultiBufferRange::new(MultiBufferOffset::ZERO, second_len)
                    .expect("完整片段范围应有效")
                    .into(),
                Vec::new(),
            )],
            cx,
        );
    });

    let (editor, cx) = cx.add_window_view({
        let combined = combined.clone();
        move |_, cx| Editor::for_multi_buffer(combined, cx)
    });
    cx.run_until_parked();
    editor.update(cx, |editor, cx| {
        editor.set_soft_wrap_mode(Some(SoftWrap::EditorWidth), cx);
    });
    cx.run_until_parked();

    let before = cx.read_entity(&editor, |editor, cx| {
        let snapshot = editor.display_snapshot(cx);
        let mut rows = snapshot.rows(DisplayRow::ZERO, snapshot.line_count());
        let mut result = Vec::new();
        while let Some(row) = rows.next() {
            if let Some(block) = row.block() {
                result.push((row.index().get(), block.excerpt.path().to_path_buf()));
            }
        }
        result
    });
    assert_eq!(
        before,
        vec![(0, PathBuf::from("a.rs")), (4, PathBuf::from("b.rs"))]
    );

    // 在第一个文件首行后插入换行：a.rs 标题不动，b.rs 标题整体后移一行。
    cx.update_entity(&first, |source, cx| {
        source
            .edit(
                [Edit::insert(MultiBufferOffset::new(14).into(), "\n").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("源编辑应成功");
    });
    cx.run_until_parked();

    let after = cx.read_entity(&editor, |editor, cx| {
        let snapshot = editor.display_snapshot(cx);
        let mut rows = snapshot.rows(DisplayRow::ZERO, snapshot.line_count());
        let mut result = Vec::new();
        while let Some(row) = rows.next() {
            if let Some(block) = row.block() {
                result.push((row.index().get(), block.excerpt.path().to_path_buf()));
            }
        }
        result
    });
    assert_eq!(
        after,
        vec![(0, PathBuf::from("a.rs")), (5, PathBuf::from("b.rs"))]
    );
}

#[gpui::test]
fn long_line_highlight_query_is_clipped_to_render_budget(cx: &mut TestAppContext) {
    // 超长单行：高亮查询只覆盖可见前缀（渲染端同样只塑形前 MAX_RENDERED_LINE_LEN 字节）。
    let long = "let text = \"".to_owned() + &"a".repeat(8192) + "\";\n";
    let buffer = Buffer::from_text(long, BufferConfig::default()).expect("测试 Buffer 应能创建");
    let buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("main.rs")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    cx.run_until_parked();

    cx.read_entity(&editor, |editor, cx| {
        let snapshot = editor.display_snapshot(cx);
        let source_ranges = snapshot.rows(DisplayRow::ZERO, 1).source_line_ranges();
        let spans = snapshot.highlighted_spans_for_source_ranges(source_ranges);
        let buffer = snapshot.buffer_snapshot();
        let second_line = buffer
            .line_start_byte(Line::new(1))
            .expect("第二行行首应存在");
        // 查询范围不超过渲染预算：spans 终点不超过第一行的前 MAX_RENDERED_LINE_LEN 字节。
        let covered = spans.iter().map(|span| span.range.end).max().unwrap_or(0);
        assert!(
            covered <= 1024 && covered < second_line.get(),
            "超长行高亮终点应在预算内，实际 {covered}（第二行行首 {}）",
            second_line.get()
        );
    });
}

#[gpui::test]
fn horizontal_windowing_clips_wide_rows_to_the_visible_window(cx: &mut TestAppContext) {
    // 未换行 + 超长行：非光标行只合成/塑形可见列窗口（±边距）内的文本，
    // 并回报窗口起点列供渲染端补偿行原点；光标行保持整行 shaping（autoscroll 依赖光标像素）。
    let long = "a".repeat(4096) + "tail";
    let buffer = Buffer::from_text(format!("{long}\n{long}\n"), BufferConfig::default())
        .expect("测试 Buffer 应能创建");
    let buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("main.rs")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    cx.run_until_parked();

    let (windowed_len, window_start, full_len) = cx.read_entity(&editor, |editor, cx| {
        let snapshot = editor.display_snapshot(cx);
        let row_metrics = |start: usize, window| {
            let mut chunks = snapshot.chunks(
                DisplayRow::new(start)..DisplayRow::new(start + 1),
                crate::display_map::HighlightStyles::default(),
                window,
            );
            let mut len = 0;
            let mut window_start = 0;
            chunks.for_each_row(|event| {
                if let crate::display_map::DisplayRowEvent::Text { row, chunks } = event {
                    window_start = row.window_start_column;
                    len += chunks.map(|chunk| chunk.text.len()).sum::<usize>();
                }
            });
            (len, window_start)
        };
        let (windowed_len, window_start) = row_metrics(0, Some((200usize, 500usize)));
        let (full_len, _) = row_metrics(1, None);
        (windowed_len, window_start, full_len)
    });
    assert!(
        windowed_len < 4096,
        "非光标超长行应被窗口化裁剪，实际 {windowed_len}"
    );
    assert_eq!(window_start, 200, "窗口化行应回报窗口起点列");
    assert_eq!(
        full_len, 1024,
        "无窗口参数时整行 shaping 受 1024 上限约束，实际 {full_len}"
    );
}

/// 回归：多文件编辑器中 cursor_text 必须显示源文件的真实行（1 起始）。
///
/// `source_start_line` 约定为 1 起始（gutter、悬浮标题直接使用），光标显示不得再次 +1，否则会显示成比实际大 1 的行号。
#[gpui::test]
fn cursor_text_maps_excerpt_output_to_real_source_line(cx: &mut TestAppContext) {
    // 源文件 8 行（0 起始 0..7）；excerpt 只取 0 起始第 5、6 行（a5 / a6）。
    let source = test_buffer(cx, "a0\na1\na2\na3\na4\na5\na6\n");
    source.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let source_multi = source;
    let combined = cx.new(MultiBuffer::empty);
    combined.update(cx, |combined, cx| {
        combined.set_excerpts_for_path(vec![ExcerptRange::line_range(source_multi, 5..7, cx)], cx);
    });
    let editor = cx.new(move |cx| Editor::for_multi_buffer(combined, cx));

    editor.update(cx, |editor, cx| {
        // excerpt 首行对应源文件第 6 行（1 起始）。
        editor.select_byte_range(0..0, cx);
        assert_eq!(editor.cursor_text(cx), "6:1");
        // 源文件第 7 行第 2 列（1 起始）。
        editor.select_byte_range(4..4, cx);
        assert_eq!(editor.cursor_text(cx), "7:2");
    });

    // 单文件文档：组合坐标即源坐标（0 起始 → 1 起始显示）。
    let single_buffer = test_buffer(cx, "x\ny\n");
    let single = cx.new(|cx| Editor::for_language_buffer(single_buffer, cx));
    single.update(cx, |editor, cx| {
        editor.select_byte_range(2..2, cx);
        assert_eq!(editor.cursor_text(cx), "2:1");
    });
}

/// 整文件作为可编辑工作区 excerpt，展开的 Deleted hunk 作为只读 HEAD excerpt 插入删除点（工作区切分拼接）：
/// 光标可停在 HEAD 行、显示修订行列、工作区可编辑写回。
#[gpui::test]
fn materialized_deleted_excerpt_keeps_editing_and_cursor(cx: &mut TestAppContext) {
    // 工作区（新侧）与 HEAD（旧侧，含被删行 old1/old2）。
    let work = test_buffer(cx, "a\nb\nc");
    work.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let head = test_buffer(cx, "a\nold1\nold2\nc");
    head.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let work_multi = work.clone();
    let head_multi = head.clone();
    let combined = cx.new(MultiBuffer::empty);

    // Deleted hunk：新侧行 1 处删除 HEAD 的 1..3 行。
    // 组合 = [工作区 0..1] + [HEAD 1..3（只读红色行）] + [工作区 1..3]。
    combined.update(cx, |combined, cx| {
        combined.set_excerpts_for_path(
            vec![
                ExcerptRange::line_range(work_multi.clone(), 0..1, cx),
                ExcerptRange::line_range(head_multi, 1..3, cx)
                    .with_diff_kind(ExcerptDiffKind::Deleted)
                    .with_editable(false),
                ExcerptRange::line_range(work_multi, 1..3, cx),
            ],
            cx,
        );
    });
    let editor = cx.new(move |cx| Editor::for_multi_buffer(combined, cx));

    // 组合文本：HEAD 旧行插在删除点；普通 excerpt 保留源文本原样（末尾无多余换行）。
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nold1\nold2\nb\nc");
    });
    // 光标可移动到 HEAD 行（组合 offset 2 = "old1"），并显示 HEAD 修订行列（第 2 行，1 起始）。
    editor.update(cx, |editor, cx| {
        editor.select_byte_range(2..2, cx);
        assert_eq!(editor.cursor_text(cx), "2:1");
        // HEAD 第二行（old2）行首：修订第 3 行。
        editor.select_byte_range(7..7, cx);
        assert_eq!(editor.cursor_text(cx), "3:1");
    });
    // 工作区 excerpt 仍可编辑并写回工作区文件（"b" → "B"）。
    editor.update(cx, |editor, cx| {
        editor.select_byte_range(12..13, cx);
        editor.replace_text(None, "B", cx);
    });
    assert_eq!(buffer_text(&work, cx), "a\nB\nc");
}

/// 回归：组合文档（git hunk 上下文裁剪）未保存删除整行后，保留既有 excerpt 与源光标。
///
/// 删除顶部上下文行会移动裁剪窗口（新行从顶部进入），投影被整体重建（reload）；
/// 编辑器若把编辑后裸偏移直接重锚到重建后的投影版本，光标会跳到错误行。
#[gpui::test]
fn combined_diff_dirty_edit_keeps_existing_excerpt_and_cursor(cx: &mut TestAppContext) {
    let working_text = "L0\nL1\nL2\nL3\nADDED\nL5\nL6\nL7\nL8\n";
    let head_text = "L0\nL1\nL2\nL3\nL5\nL6\nL7\nL8\n";

    // 组合文档：Added hunk 在源第 4 行，context_lines=2 → 初始只显示源行 [2..7)。
    let source = test_buffer(cx, working_text);
    source.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let combined_source = source.clone();
    let combined = cx.new(MultiBuffer::empty);
    combined.update(cx, |combined, cx| {
        combined.set_diff_files(vec![clipped_diff_file(combined_source, head_text, cx)], cx);
    });
    // diff 后台计算完成后投影才可用；组合文档断言前等待落定。
    cx.run_until_parked();
    let editor = cx.new(move |cx| Editor::for_multi_buffer(combined, cx));

    // 初始投影：源行 [2..7) = "L2 L3 ADDED L5 L6"。
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "L2\nL3\nADDED\nL5\nL6\n");
    });

    // 删除首个可见行 "L2\n"（投影 offset 0..3 → 源 offset 6..9）。
    editor.update(cx, |editor, cx| {
        editor.select_byte_range(0..3, cx);
        editor.replace_text(None, "", cx);
    });
    // dirty source 只更新既有 excerpt 的文本，不重新计算上下文窗口；断言前等待落定。
    cx.run_until_parked();

    // 源被正确编辑；既有源范围 [2..7) 现在显示为 L3/ADDED/L5/L6。
    assert_eq!(
        buffer_text(&source, cx),
        "L0\nL1\nL3\nADDED\nL5\nL6\nL7\nL8\n"
    );
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "L3\nADDED\nL5\nL6\n");
        // 光标仍绑定删除后的源位置；既有 excerpt 的显示起点现在是 offset 0。
        let selections = editor.selections(cx);
        let caret = selections.primary();
        assert!(caret.is_caret(), "删除后应为单光标");
        assert_eq!(caret.head(), MultiBufferOffset::ZERO);
    });
}

/// 回归：组合文档折叠/展开 hunk 触发投影整体重建后，光标必须停在同一逻辑源位置，而不是被重置到投影开头——与普通编辑器折叠不移动光标的行为保持一致。
#[gpui::test]
fn combined_diff_toggle_hunk_keeps_cursor_at_same_source_position(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\nb\nc");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    let source = buffer.clone();
    inject_editor_diff(
        &editor,
        &source,
        vec![DisplayHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
            staging: DiffHunkStaging::NoStaging,
        }],
        Some(Arc::from("a\nold1\nold2\nc")),
        cx,
    );

    // 折叠态：光标停在工作区源 "b" 行首（源 offset 2，折叠投影 offset 2）。
    editor.update(cx, |editor, cx| editor.select_byte_range(2..2, cx));
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nb\nc");
        let selections = editor.selections(cx);
        assert_eq!(selections.primary().head(), MultiBufferOffset::new(2));
    });

    // 展开 Deleted hunk：只读旧行插入删除点，投影整体重建（reload，版本重置）。
    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    cx.run_until_parked();

    // 工作区源未变，"b" 行首仍是源 offset 2；新投影中它落在 offset 12。
    // 光标必须跟随源位置到 12，而不是被重建重置到投影开头 offset 0（"a" 行首）。
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nold1\nold2\nb\nc");
        let selections = editor.selections(cx);
        let caret = selections.primary();
        assert!(caret.is_caret(), "折叠/展开后应保持单光标");
        assert_eq!(caret.head(), MultiBufferOffset::new(12));
    });
}

/// 回归：组合文档下工作区源被外部编辑（非经编辑器）时，光标必须像普通编辑器一样跟随到同一逻辑源位置，而不是被投影重建重置到开头——与普通编辑器 external_reload 行为一致（参见 actions_tests::external_reload_moves_selection_through_diff）。
#[gpui::test]
fn external_source_edit_moves_combined_diff_cursor_like_plain_editor(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "alpha\nbravo\ncharlie");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    let source = buffer.clone();
    inject_editor_diff(
        &editor,
        &source,
        vec![DisplayHunk {
            range: 2..2,
            old_range: 2..3,
            kind: DiffHunkKind::Deleted,
            staging: DiffHunkStaging::NoStaging,
        }],
        Some(Arc::from("alpha\nbravo\nold\ncharlie")),
        cx,
    );

    // 折叠态投影显示整个工作区文件；光标停在 "charlie" 行内 "ch" 之后（投影 offset 14）。
    editor.update(cx, |editor, cx| editor.select_byte_range(14..14, cx));
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "alpha\nbravo\ncharlie");
        assert_eq!(
            editor.selections(cx).primary().head(),
            MultiBufferOffset::new(14)
        );
    });

    // 外部在 "bravo" 与 "charlie" 之间插入整行 "NEW"：hunk 位置随源下移，组合投影按增量更新。
    cx.update_entity(&buffer, |buffer, cx| {
        buffer
            .replace_text("alpha\nbravo\nNEW\ncharlie".to_owned(), cx)
            .expect("外部 reload 应成功");
    });
    cx.run_until_parked();

    // 光标必须跟随 "charlie" 下移 4 字节到 offset 18（源忠实），而不是被重建重置到投影开头。
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "alpha\nbravo\nNEW\ncharlie");
        let selections = editor.selections(cx);
        let caret = selections.primary();
        assert!(caret.is_caret(), "外部编辑后应保持单光标");
        assert_eq!(caret.head(), MultiBufferOffset::new(18));
    });
}

/// 回归：组合文档删除可见行触发裁剪窗口移动 + 投影重建后，undo/redo 必须把光标恢复到编辑前/后的同一逻辑位置，而不是被重建重置——与普通编辑器 undo/redo 光标行为一致。
#[gpui::test]
fn combined_diff_undo_redo_restores_cursor_parity_with_plain_editor(cx: &mut TestAppContext) {
    let working_text = "L0\nL1\nL2\nL3\nADDED\nL5\nL6\nL7\nL8\n";
    let head_text = "L0\nL1\nL2\nL3\nL5\nL6\nL7\nL8\n";
    let source = test_buffer(cx, working_text);
    source.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let combined_source = source.clone();
    let combined = cx.new(MultiBuffer::empty);
    combined.update(cx, |combined, cx| {
        combined.set_diff_files(vec![clipped_diff_file(combined_source, head_text, cx)], cx);
    });
    // diff 后台计算完成后投影才可用；组合文档断言前等待落定。
    cx.run_until_parked();
    let editor = cx.new(move |cx| Editor::for_multi_buffer(combined, cx));

    // 删除首个可见行 "L2\n"（投影 0..3）：裁剪窗口上移、投影整体重建，光标落在新投影 offset 3。
    editor.update(cx, |editor, cx| {
        editor.select_byte_range(0..3, cx);
        editor.replace_text(None, "", cx);
    });
    // 编辑后 diff 在后台重算并重新裁剪上下文窗口；断言前等待落定。
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "L3\nADDED\nL5\nL6\n");
        assert_eq!(
            editor.selections(cx).primary().head(),
            MultiBufferOffset::ZERO
        );
    });

    // undo：源与裁剪窗口都回到编辑前，光标恢复为编辑前选区（投影 0..3），而不是被重建重置到开头。
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    cx.run_until_parked();
    assert_eq!(buffer_text(&source, cx), working_text);
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "L2\nL3\nADDED\nL5\nL6\n");
        let selections = editor.selections(cx);
        assert_eq!(selections.primary().tail(), MultiBufferOffset::new(0));
        assert_eq!(selections.primary().head(), MultiBufferOffset::new(3));
    });

    // redo：再次删除；脏文档保留当前投影，光标回到投影起点。
    cx.update_entity(&editor, |editor, cx| editor.redo(cx));
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "L3\nADDED\nL5\nL6\n");
        let selections = editor.selections(cx);
        let caret = selections.primary();
        assert!(caret.is_caret(), "redo 后应为删除落点的单光标");
        assert_eq!(caret.head(), MultiBufferOffset::ZERO);
    });
}

/// 单文件编辑器与组合文档编辑器共用 diff projection 和 hunk 展开行为。
#[gpui::test]
fn single_file_diff_expansion_uses_composite_projection(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\nb\nc");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    // 单文件编辑器也通过 MultiBuffer diff projection 注入 base 文本。
    let source = buffer.clone();
    inject_file_diff(&editor, &source, Arc::from("a\nold1\nold2\nb\nc"), cx);
    cx.refresh().expect("单文件 diff projection 应能刷新");
    let (window_bounds, line_height) =
        cx.update(|window, _| (window.bounds(), window.line_height()));
    // 点击同一条 gutter 色带，使用与组合文档完全相同的命中路径展开旧侧文本。
    cx.simulate_mouse_down(
        point(
            window_bounds.left() + px(1.),
            window_bounds.top() + line_height * 1.5,
        ),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nold1\nold2\nb\nc");
        assert_eq!(editor.diff_hunk_expanded(cx), vec![true]);
        assert_eq!(editor.diff_hunk_old_ranges(cx), &[Some(1..3)]);
    });

    // 展开态是编辑器投影状态；编辑工作区源后，hunk 仍应保持展开，
    // 而不是由外部重新注入 diff 把投影折叠回去。
    editor.update(cx, |editor, cx| {
        editor.multi_buffer.update(cx, |buffer, cx| {
            buffer
                .edit(
                    vec![Edit::insert(MultiBufferOffset::new(12).into(), "edited\n").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .expect("展开 hunk 后的源编辑应成功");
        });
    });
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, cx| {
        assert!(
            editor
                .diff_hunk_expanded(cx)
                .iter()
                .all(|&expanded| expanded)
        );
        assert!(editor.text(cx).contains("old1\nold2\nedited\n"));
    });

    // 再次点击恢复折叠态，工作区正文仍可继续编辑。
    cx.simulate_mouse_down(
        point(
            window_bounds.left() + px(1.),
            window_bounds.top() + line_height * 1.5,
        ),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nedited\nb\nc");
        assert_eq!(editor.diff_hunk_expanded(cx), vec![false]);
    });
}

/// 折叠的删除块三角锚点：删除第 17 行（1-based，0-based 16）后，锚点行必须是组合 0-based 16 行（16/17 行边界），不能是 15 或 17。
#[gpui::test]
fn folded_deleted_hunk_anchor_is_at_the_deletion_row_boundary(cx: &mut TestAppContext) {
    // 工作区已删除第 17 行（1-based）：新侧 19 行，base 20 行。
    let working_text = (1..=20)
        .filter(|line| *line != 17)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let base_text = (1..=20)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    let buffer = test_buffer(cx, working_text.clone());
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx)
    });
    let source = buffer.clone();
    inject_editor_diff(&editor, &source, Vec::new(), Some(Arc::from(base_text)), cx);
    cx.run_until_parked();

    cx.read_entity(&editor, |editor, cx| {
        // 折叠态：组合保持新侧 19 行（普通编辑器整文件模式）。
        assert_eq!(editor.text(cx), working_text);
        let snapshot = editor.display_snapshot(cx);
        let rendering = hunk_rendering(
            &snapshot,
            resolved_hunks(
                editor.diff_hunks(cx),
                editor.diff_hunk_expanded(cx),
                editor.diff_hunk_old_ranges(cx),
                editor.diff_hunk_word_diffs(cx),
            )
            .into_iter(),
        );
        assert_eq!(
            rendering.hit_regions,
            vec![(16..17, 0, DiffHunkKind::Deleted)],
            "折叠删除块锚点应在组合 0-based 16 行（16/17 行边界）"
        );
    });
}

/// 回归：diff 刷新移除已展开 hunk 时，旧侧只读 excerpt 也必须随权威 hunk 数据消失。
#[gpui::test]
fn refreshing_diff_hunks_removes_stale_expanded_excerpt(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\nb\nc");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    let source = buffer.clone();
    inject_editor_diff(
        &editor,
        &source,
        Vec::new(),
        Some(Arc::from("a\nold1\nold2\nb\nc")),
        cx,
    );
    editor.update(cx, |editor, cx| {
        editor.toggle_diff_hunk_at(0, cx);
        assert_eq!(editor.text(cx), "a\nold1\nold2\nb\nc");
    });
    // 重新注入 base 为空：整份文本变为 Added，旧侧只读 excerpt 必须消失。
    inject_editor_diff(&editor, &source, Vec::new(), None, cx);
    editor.update(cx, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nb\nc");
        assert!(editor.diff_hunk_old_ranges(cx).iter().all(Option::is_none));
    });
}

/// 回归：diff 刷新后 hunk 边界变化（编辑导致 hunk 合并/移位）时，展开状态按旧侧行范围锚点迁移到新 hunk，不再因 old_range 精确匹配失败而丢失用户的显式展开。
#[gpui::test]
fn refreshing_diff_hunks_migrates_expansion_across_hunk_boundary_changes(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\nb\nc");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    let source = buffer.clone();
    inject_editor_diff(
        &editor,
        &source,
        Vec::new(),
        Some(Arc::from("a\nold1\nold2\nb\nc")),
        cx,
    );
    editor.update(cx, |editor, cx| {
        editor.toggle_diff_hunk_at(0, cx);
        assert_eq!(editor.text(cx), "a\nold1\nold2\nb\nc");
    });

    // base 变化使旧侧范围扩大（1..3 → 1..4）：展开状态按工作区锚点迁移到新 hunk。
    inject_editor_diff(
        &editor,
        &source,
        Vec::new(),
        Some(Arc::from("a\nold1\nold2\nold3\nb\nc")),
        cx,
    );
    editor.update(cx, |editor, cx| {
        assert!(
            editor
                .diff_hunk_expanded(cx)
                .iter()
                .all(|&expanded| expanded),
            "展开状态应按旧侧行范围锚点迁移到新 hunk"
        );
        assert_eq!(editor.text(cx), "a\nold1\nold2\nold3\nb\nc");
    });
}

/// 回归：diff 刷新只清理真正消失的 hunk 状态，仍存在的 hunk 展开状态按锚点保留。
#[gpui::test]
fn refreshing_diff_hunks_drops_state_of_disappeared_hunk_only(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\nb\nc\nd\ne");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    let source = buffer.clone();
    inject_editor_diff(
        &editor,
        &source,
        Vec::new(),
        Some(Arc::from("a\nold1\nb\nc\nold3\nd\ne")),
        cx,
    );
    editor.update(cx, |editor, cx| {
        editor.toggle_diff_hunk_at(0, cx);
        editor.toggle_diff_hunk_at(1, cx);
        assert_eq!(editor.text(cx), "a\nold1\nb\nc\nold3\nd\ne");
    });

    // 第一个删除块对应的改动被还原：该 hunk 消失，仅清理其状态；第二个保留。
    inject_editor_diff(
        &editor,
        &source,
        Vec::new(),
        Some(Arc::from("a\nb\nc\nold3\nd\ne")),
        cx,
    );
    editor.update(cx, |editor, cx| {
        assert!(
            editor
                .diff_hunk_expanded(cx)
                .iter()
                .all(|&expanded| expanded)
        );
        assert_eq!(editor.text(cx), "a\nb\nc\nold3\nd\ne");
    });
}

/// 回归：默认展开模式（项目差异视图）下，用户显式折叠的 hunk 在刷新后按锚点迁移保留。
#[gpui::test]
fn refreshing_diff_hunks_preserves_collapsed_hunk_in_default_expanded_mode(
    cx: &mut TestAppContext,
) {
    let buffer = test_buffer(cx, "a\nb\nc");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    editor.update(cx, |editor, cx| {
        editor.set_diff_hunks_expanded_by_default(true, cx)
    });
    let source = buffer.clone();
    inject_editor_diff(
        &editor,
        &source,
        vec![DisplayHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
            staging: DiffHunkStaging::NoStaging,
        }],
        Some(Arc::from("a\nold1\nold2\nc")),
        cx,
    );
    editor.update(cx, |editor, cx| {
        editor.toggle_diff_hunk_at(0, cx);
        assert!(
            !editor
                .diff_hunk_expanded(cx)
                .iter()
                .any(|&expanded| expanded),
            "默认展开模式下显式折叠后不应再展开"
        );
    });

    // 边界变化后折叠状态迁移到新 hunk。
    inject_editor_diff(
        &editor,
        &source,
        vec![DisplayHunk {
            range: 1..1,
            old_range: 1..4,
            kind: DiffHunkKind::Deleted,
            staging: DiffHunkStaging::NoStaging,
        }],
        Some(Arc::from("a\nold1\nold2\nc")),
        cx,
    );
    editor.update(cx, |editor, cx| {
        assert!(
            !editor
                .diff_hunk_expanded(cx)
                .iter()
                .any(|&expanded| expanded),
            "折叠状态应按旧侧行范围锚点迁移到新 hunk"
        );
    });
}

/// 回归：base 版本变化（提交等）后宿主重置展开状态，新 hunk 按默认策略重新注入，
/// 已物化的旧侧 excerpt 同时撤销。
#[gpui::test]
fn reset_diff_hunk_expansion_state_restores_default_strategy(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\nb\nc");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    let source = buffer.clone();
    inject_editor_diff(
        &editor,
        &source,
        vec![DisplayHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
            staging: DiffHunkStaging::NoStaging,
        }],
        Some(Arc::from("a\nold1\nold2\nc")),
        cx,
    );
    editor.update(cx, |editor, cx| {
        editor.toggle_diff_hunk_at(0, cx);
        assert_eq!(editor.text(cx), "a\nold1\nold2\nb\nc");

        editor.reset_diff_hunk_expansion_state(cx);
        assert!(
            !editor
                .diff_hunk_expanded(cx)
                .iter()
                .any(|&expanded| expanded)
        );
        assert_eq!(
            editor.text(cx),
            "a\nb\nc",
            "重置后应撤销已物化的旧侧 excerpt"
        );
    });
}

/// 回归：普通编辑器把修改块旧侧物化为 MultiBuffer excerpt 后，必须与多文件编辑器
/// 消费同一份物化 hunk 映射；旧侧行是删除色，新侧行是新增色，gutter 色带覆盖两侧。
#[gpui::test]
fn plain_editor_expanded_modified_hunk_keeps_old_rows_and_gutter_strip(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\nnew\nc");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx)
    });
    let source = buffer.clone();
    inject_editor_diff(
        &editor,
        &source,
        vec![DisplayHunk {
            range: 1..2,
            old_range: 1..2,
            kind: DiffHunkKind::Modified,
            staging: DiffHunkStaging::NoStaging,
        }],
        Some(Arc::from("a\nold\nc")),
        cx,
    );
    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));

    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nold\nnew\nc");
        let snapshot = editor.display_snapshot(cx);
        let rendering = hunk_rendering(
            &snapshot,
            resolved_hunks(
                editor.diff_hunks(cx),
                editor.diff_hunk_expanded(cx),
                editor.diff_hunk_old_ranges(cx),
                editor.diff_hunk_word_diffs(cx),
            )
            .into_iter(),
        );
        assert_eq!(
            rendering.diff_rows,
            vec![
                (1..2, DiffHunkKind::Deleted, DiffHunkStaging::NoStaging),
                (2..3, DiffHunkKind::Added, DiffHunkStaging::NoStaging),
            ],
            "展开的普通编辑器修改块应保留旧侧红色行和新侧绿色行"
        );
        assert_eq!(
            rendering.strips,
            vec![(1..3, DiffHunkKind::Modified, DiffHunkStaging::NoStaging)],
            "gutter 色带应覆盖修改块的旧侧与新侧"
        );
        assert_eq!(
            rendering.hit_regions,
            vec![(1..3, 0, DiffHunkKind::Modified)],
            "展开的修改块仍应暴露 gutter 折叠点击区"
        );
    });

    cx.run_until_parked();
    cx.refresh().expect("展开修改块后应能刷新");
    let (window_bounds, line_height) =
        cx.update(|window, _| (window.bounds(), window.line_height()));
    cx.simulate_mouse_down(
        point(
            window_bounds.left() + px(1.),
            window_bounds.top() + line_height * 1.5,
        ),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nnew\nc");
        assert!(
            !editor
                .diff_hunk_expanded(cx)
                .iter()
                .any(|&expanded| expanded),
            "点击展开块的 gutter 色带应折叠 hunk"
        );
    });
}

/// 回归：diff 旧侧 excerpt 展开后又因暂存刷新而移除时，显示行 chunk 始终只携带行内容。
///
/// 行终止符属于组合文本的坐标事实；如果它穿透到 `DisplayRowEvent::Text`，单行
/// shaping 会触发 GPUI 的 `text argument should not contain newlines` 断言。
#[gpui::test]
fn diff_refresh_keeps_line_terminators_out_of_renderer_chunks(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\r\nnew\r\nc\r\n");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx)
    });
    let source = buffer.clone();
    inject_editor_diff(
        &editor,
        &source,
        Vec::new(),
        Some(Arc::from("a\r\nold\r\nc\r\n")),
        cx,
    );
    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    cx.run_until_parked();

    // 暂存当前 hunk 后，未暂存视图的 index 基线与工作区一致，旧侧 excerpt 被移除。
    inject_editor_diff(
        &editor,
        &source,
        Vec::new(),
        Some(Arc::from("a\r\nnew\r\nc\r\n")),
        cx,
    );

    let rendered_rows = cx.read_entity(&editor, |editor, cx| {
        let snapshot = editor.display_snapshot(cx);
        let mut chunks = snapshot.chunks(
            DisplayRow::ZERO..DisplayRow::new(snapshot.line_count()),
            crate::display_map::HighlightStyles::default(),
            None,
        );
        let mut rows = Vec::new();
        chunks.for_each_row(|event| {
            if let crate::display_map::DisplayRowEvent::Text { chunks, .. } = event {
                rows.push(chunks.map(|chunk| chunk.text).collect::<String>());
            }
        });
        rows
    });
    assert_eq!(rendered_rows, ["a", "new", "c", ""]);
    assert!(rendered_rows.iter().all(|row| !row.contains(['\r', '\n'])));
    cx.refresh().expect("暂存刷新后的 diff 视图应能完成布局");
}

/// 回归：软换行开启、暂存 hunk 触发结构变更时，Wrap 的 Tab 点输入边界必须保持自洽。
#[gpui::test]
fn staging_a_hunk_with_soft_wrap_keeps_wrap_map_invariant(cx: &mut TestAppContext) {
    let fill = "x".repeat(120);
    let mut working = String::new();
    let mut base = String::new();
    for index in 0..1500 {
        let (working_marker, base_marker) = if index == 750 {
            ("new", "old")
        } else {
            ("line", "line")
        };
        working.push_str(&format!("{working_marker} {index} {fill}\r\n"));
        base.push_str(&format!("{base_marker} {index} {fill}\r\n"));
    }
    let buffer = test_buffer(cx, &working);
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx)
    });
    cx.run_until_parked();
    cx.update_entity(&editor, |editor, cx| {
        editor.set_soft_wrap_mode(Some(SoftWrap::EditorWidth), cx);
    });
    cx.run_until_parked();
    cx.refresh().expect("软换行模式下的首帧应能完成布局");
    assert!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .is_wrapped()),
        "软换行模式必须让 WrapMap 进入带测量宽度的同步路径"
    );
    let source = buffer.clone();
    inject_editor_diff(&editor, &source, Vec::new(), Some(Arc::from(base)), cx);
    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    cx.run_until_parked();
    cx.refresh().expect("展开 hunk 后的软换行帧应能完成布局");
    assert!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .is_wrapped()),
        "展开 hunk 后软换行必须保留"
    );
    // 暂存后重新注入：旧侧 excerpt 被移除，工作区与 index 基线一致。
    inject_editor_diff(&editor, &source, Vec::new(), Some(Arc::from(working)), cx);
    cx.run_until_parked();
    cx.refresh()
        .expect("暂存刷新后的软换行 diff 视图应能完成布局");
    assert!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .is_wrapped()),
        "暂存 hunk 后组合文档的软换行必须保留"
    );
}

/// 回归：在只读的 Deleted 旧行上尝试编辑（被拒）后，光标移回工作区仍可正常编辑。
#[gpui::test]
fn editing_readonly_deleted_row_then_editing_working_text_still_works(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\nb\nc");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    let source = buffer.clone();
    inject_editor_diff(
        &editor,
        &source,
        vec![DisplayHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
            staging: DiffHunkStaging::NoStaging,
        }],
        Some(Arc::from("a\nold1\nold2\nc")),
        cx,
    );
    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    cx.run_until_parked();

    // 光标在只读的 HEAD 旧行（组合 offset 2 = "old1"）上尝试替换：应被拒绝。
    editor.update(cx, |editor, cx| {
        editor.select_byte_range(2..3, cx);
        editor.replace_text(None, "X", cx);
    });
    // 光标移回工作区（"b"）并编辑：必须仍然生效。
    editor.update(cx, |editor, cx| {
        editor.select_byte_range(12..13, cx);
        editor.replace_text(None, "B", cx);
    });
    assert_eq!(buffer_text(&buffer, cx), "a\nB\nc");
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nold1\nold2\nB\nc");
    });
}

/// 带文件路径的测试源：diff 旧侧源复用同一路径，展开旧侧时路径身份才能一致。
fn test_file_buffer(cx: &mut TestAppContext, path: &str, text: &str) -> Entity<LanguageBuffer> {
    let buffer =
        Buffer::from_text(text.to_string(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from(path)),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    })
}

/// 读取主光标所在投影偏移对应的源位置。
fn caret_source_range(editor: &Entity<Editor>, cx: &TestAppContext) -> zcv_text::TextRange {
    cx.read_entity(editor, |editor, cx| {
        let caret = editor.selections(cx).primary().head();
        editor
            .multi_buffer()
            .read(cx)
            .location_for_offset(caret)
            .expect("光标必须落在可见 excerpt 内")
            .source_range
    })
}

/// 回归：diff 展开/折叠只重建投影拓扑，选区按源 Anchor 解析到同一源位置。
#[gpui::test]
fn diff_expansion_preserves_selection_source_anchor(cx: &mut TestAppContext) {
    let source = test_file_buffer(cx, "src/a.rs", "a\nworking\nc\n");
    let editor = cx.new(|cx| Editor::from_language_buffer(source.clone(), EditorMode::Full, cx));
    inject_file_diff(&editor, &source, Arc::from("a\nold\nc\n"), cx);

    editor.update(cx, |editor, cx| {
        editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(2)), cx);
    });
    let before = caret_source_range(&editor, cx);

    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    cx.run_until_parked();

    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor.diff_hunk_expanded(cx)),
        vec![true],
        "切换后 hunk 应处于展开态"
    );
    assert_eq!(
        caret_source_range(&editor, cx),
        before,
        "展开/折叠重建投影后，选区源位置不得改变"
    );
}

/// 回归：外部源变更后选区源 Anchor 按当前快照解析，重建投影后仍落在同一逻辑源位置。
#[gpui::test]
fn external_source_change_advances_selection_source_anchor(cx: &mut TestAppContext) {
    let source = test_file_buffer(cx, "src/a.rs", "a\nworking\nc\n");
    let editor = cx.new(|cx| Editor::from_language_buffer(source.clone(), EditorMode::Full, cx));
    inject_file_diff(&editor, &source, Arc::from("a\nold\nc\n"), cx);

    editor.update(cx, |editor, cx| {
        editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(2)), cx);
    });
    assert_eq!(
        caret_source_range(&editor, cx),
        zcv_text::TextRange::new(ByteOffset::new(2), ByteOffset::new(2)).unwrap()
    );

    // 外部（未经本编辑器）在源开头插入 "prefix\n"，光标源位置应随源变更右移 7 字节。
    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                [Edit::insert(ByteOffset::ZERO, "prefix\n").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("外部源编辑应成功");
        cx.notify();
    });
    cx.run_until_parked();

    assert_eq!(
        caret_source_range(&editor, cx),
        zcv_text::TextRange::new(ByteOffset::new(9), ByteOffset::new(9)).unwrap(),
        "外部源变更后选区源 Anchor 应按当前快照解析到同一逻辑位置"
    );
}

/// 只有空文本且设置了 placeholder 时才返回提示快照；判空走当前快照，不在渲染帧物化整份文本。
#[gpui::test]
fn placeholder_snapshot_requires_empty_text(cx: &mut TestAppContext) {
    let editor = cx.new(Editor::single_line);
    cx.update_entity(&editor, |editor, cx| {
        editor.set_placeholder_text("输入内容", cx);
    });
    assert!(
        cx.read_entity(&editor, |editor, cx| editor
            .placeholder_snapshot_if_empty(cx))
            .is_some(),
        "空文本应返回 placeholder 快照"
    );
    cx.update_entity(&editor, |editor, cx| {
        editor.set_text("内容", cx);
    });
    assert!(
        cx.read_entity(&editor, |editor, cx| editor
            .placeholder_snapshot_if_empty(cx))
            .is_none(),
        "非空文本不得返回 placeholder 快照"
    );
}

#[gpui::test]
fn folding_a_section_with_soft_wrap_enabled_keeps_wrap_map_invariant(cx: &mut TestAppContext) {
    // 软换行先开启、再折叠：结构编辑必须让 Wrap 变换树的 input 行数等于折叠后的 tab 行数。
    let text = "# 架构决策记录\n\n第一段正文。\n\n第二段正文。\n\n第三段正文。\n";
    let buffer = Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("Buffer");
    let source = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("docs/架构决策记录.md")),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let (editor, cx) = cx.add_window_view({
        let source = source.clone();
        move |_, cx| Editor::from_language_buffer(source, EditorMode::Full, cx)
    });
    cx.run_until_parked();
    cx.update_entity(&editor, |editor, cx| {
        editor.set_soft_wrap_mode(Some(SoftWrap::EditorWidth), cx);
    });
    cx.run_until_parked();
    cx.update_entity(&editor, |editor, cx| {
        editor.toggle_fold_at_line(Line::ZERO, cx);
    });
    cx.run_until_parked();
    assert!(
        cx.read_entity(&editor, |editor, cx| editor
            .display_snapshot(cx)
            .fold_anchor_lines()
            .contains(&Line::ZERO)),
        "折叠入口行应保持折叠"
    );
}
