use gpui::{Modifiers, MouseButton, TestAppContext, point, px};
use std::path::PathBuf;
use zcv_git::{DiffHunk, DiffHunkKind};
use zcv_multi_buffer::{MultiBuffer, MultiBufferExcerpt};
use zcv_text::{ByteOffset, Edit, TextRange, TransactionMetadata};

use super::common::{buffer_text, engine_buffer, inject_editor_diff, test_buffer};
use super::*;
use crate::display_map::{ProjectedLineIndex, ProjectedPoint, WrapViewportRowKind};

struct OccludingHunkControls;

impl DiffHunkDelegate for OccludingHunkControls {
    fn render_hunk_controls(
        &self,
        _row: usize,
        _hunk: &DiffHunk,
        line_height: Pixels,
        _editor: &Entity<Editor>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        div()
            .id("test-hunk-controls")
            .debug_selector(|| "test-hunk-controls".into())
            .w(px(80.))
            .h(line_height)
            .occlude()
            .into_any_element()
    }
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
    inject_editor_diff(
        &editor,
        &source,
        vec![DiffHunk {
            range: 1..2,
            old_range: 1..1,
            kind: DiffHunkKind::Added,
        }],
        None,
        cx,
    );
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
        vec![DiffHunk {
            range: 1..50,
            old_range: 1..1,
            kind: DiffHunkKind::Added,
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
    let base_rows = cx.read_entity(&editor, |editor, _| editor.display_map.line_count());
    assert_eq!(base_rows, 3);

    // 注入 Deleted hunk（新侧行 1 处删除了 HEAD 的 1..3 行）+ HEAD 全文。
    inject_editor_diff(
        &editor,
        &source,
        vec![DiffHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
        }],
        Some(Arc::from("a\nold1\nold2\nc")),
        cx,
    );
    // 未展开：行数不变。
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
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
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
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
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        3,
        "点击 gutter 折叠后应回到 3 行"
    );
}

#[gpui::test]
fn toggle_fold_collapses_and_expands_the_cursor_block(cx: &mut TestAppContext) {
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建")
    });
    let buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    cx.run_until_parked();
    // 语法解析完成后语言层提供两个折叠范围（fn main 与 fn other 的块体）。
    let fold_ranges = cx.read_entity(&editor, |editor, _| {
        editor
            .fold_ranges()
            .iter()
            .map(|range| range.range.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(fold_ranges.len(), 2);

    // 折叠 fn main（入口行 0）：隐藏块内 2 行，无占位行，总行数 6 → 4。
    editor.update(cx, |editor, cx| editor.toggle_fold_at_line(Line::ZERO, cx));
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        4
    );
    assert!(cx.read_entity(&editor, |editor, _| {
        editor
            .display_map
            .snapshot()
            .fold_anchor_lines()
            .contains(&Line::ZERO)
    }));

    // 再次切换：展开，恢复 6 行。
    editor.update(cx, |editor, cx| editor.toggle_fold_at_line(Line::ZERO, cx));
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        6
    );
    assert!(!cx.read_entity(&editor, |editor, _| {
        editor
            .display_map
            .snapshot()
            .fold_anchor_lines()
            .contains(&Line::ZERO)
    }));
}

#[gpui::test]
fn toggle_fold_action_uses_the_cursor_block_and_the_whole_folded_row(cx: &mut TestAppContext) {
    let text = "fn main() {\n    if true {\n        let x = 1;\n    }\n}\nfn other() {}";
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建")
    });
    let language_buffer =
        cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let editor = cx.new(|cx| Editor::from_language_buffer(language_buffer, EditorMode::Full, cx));
    cx.run_until_parked();

    // 光标在 if 块内部时，折叠包含它的最内层范围，而不要求位于 crease 所在行。
    editor.update(cx, |editor, cx| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(
            text.find("let x").expect("测试文本应包含 let x"),
        )));
        editor.toggle_fold_at_cursor(cx);
    });
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        4,
        "应只折叠内层 if 块"
    );

    // 光标位于折叠占位符之后的闭合尾段时，仍按同一显示行展开。
    editor.update(cx, |editor, cx| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(
            text.find("    }\n}").expect("测试文本应包含内层闭合括号") + 4,
        )));
        editor.toggle_fold_at_cursor(cx);
    });
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        6,
        "折叠合并行任意位置都应能展开"
    );
}

#[gpui::test]
fn clicking_the_crease_toggles_fold_without_selecting_the_line(cx: &mut TestAppContext) {
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建")
    });
    let language_buffer =
        cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let (editor, cx) = cx.add_window_view({
        let language_buffer = language_buffer.clone();
        move |_, cx| Editor::from_language_buffer(language_buffer, EditorMode::Full, cx)
    });
    cx.run_until_parked();

    let initial_selection = SelectionSet::caret(ByteOffset::new(3));
    editor.update(cx, |editor, _| {
        editor.set_selections(initial_selection.clone());
    });

    // 默认测试字体下，首行 crease 位于 gutter 右侧的折叠指示列中心。
    cx.simulate_click(point(px(54.), px(12.)), gpui::Modifiers::default());

    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.display_map.line_count(),
            4,
            "点击 crease 应折叠首个函数"
        );
        assert_eq!(
            editor.selections(),
            initial_selection,
            "crease 点击不应继续冒泡成 gutter 整行选择"
        );
    });
}

#[gpui::test]
fn fold_ranges_survive_edits_and_folded_state_follows(cx: &mut TestAppContext) {
    // 回归：编辑后折叠范围与折叠状态必须保持（crease 箭头显示依赖 fold_ranges / fold_anchor_lines）。
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建")
    });
    let language_buffer =
        cx.new(|cx| LanguageBuffer::new(buffer.clone(), Some(PathBuf::from("main.rs")), cx));
    let editor = cx.new(|cx| Editor::from_language_buffer(language_buffer, EditorMode::Full, cx));
    cx.run_until_parked();

    // 编辑 buffer：在首行后插入一行注释。
    cx.update_entity(&buffer, |buffer, cx| {
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(7), "// 注释\n").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("插入应成功");
        cx.notify();
    });
    cx.run_until_parked();

    // 编辑后语言层折叠范围仍可用（插值树版本与 buffer 同步）。
    let fold_ranges = cx.read_entity(&editor, |editor, _| {
        editor
            .fold_ranges()
            .iter()
            .map(|range| range.range.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(fold_ranges.len(), 2, "编辑后折叠范围应保持两个");

    // 注释行插入后 `{` 落到行 1（fold 范围起点行随编辑推进），入口行折叠仍可用。
    editor.update(cx, |editor, cx| {
        editor.toggle_fold_at_line(Line::new(1), cx)
    });
    assert!(cx.read_entity(&editor, |editor, _| {
        editor
            .display_map
            .snapshot()
            .fold_anchor_lines()
            .contains(&Line::new(1))
    }));
}
#[gpui::test]
fn folded_bracket_highlight_lands_on_merged_row(cx: &mut TestAppContext) {
    // 回归：折叠块后光标在入口行 `{` 上，另一半括号高亮投影到合并行的真实 `}` 列。
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建")
    });
    let buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    cx.run_until_parked();
    editor.update(cx, |editor, cx| editor.toggle_fold_at_line(Line::ZERO, cx));

    // 光标在 `{` 上（字节 10；字节 8/9 会命中 `()` 对）。
    let close_range = cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(10)));
        let pair = editor
            .matching_bracket_pair()
            .expect("光标旁的括号应由 tree-sitter query 匹配");
        pair.close.clone()
    });
    // 合并行文本：anchor + 占位符 + 真实 `}`。
    let snapshot = cx.read_entity(&editor, |editor, _| editor.display_map.snapshot());
    let viewport = snapshot
        .slice_viewport(DisplayRow::ZERO, 1)
        .expect("视口应可读取");
    let WrapViewportRowKind::Text { text, .. } = viewport.rows()[0].kind();
    assert_eq!(text.as_ref(), "fn main() {…}\n");
    // 真实 `}` 范围投影到合并行占位符之后的列（anchor 11 字符 + 占位符 1 列 = 12）。
    let projected = snapshot
        .project_text_range(
            zcv_text::TextRange::new(
                ByteOffset::new(close_range.start),
                ByteOffset::new(close_range.end),
            )
            .expect("`}` 范围应合法"),
        )
        .expect("投影应成功");
    assert_eq!(projected.len(), 1);
    assert_eq!(
        projected[0].start(),
        ProjectedPoint::new(ProjectedLineIndex::new(0), zcv_text::LogicalColumn::new(12))
    );
    assert_eq!(
        projected[0].end(),
        ProjectedPoint::new(ProjectedLineIndex::new(0), zcv_text::LogicalColumn::new(13))
    );
}

#[gpui::test]
fn horizontal_movement_jumps_over_folded_content(cx: &mut TestAppContext) {
    // 折叠在显示上占一个字符：右箭头从折叠起点一步跨到闭合括号，左箭头回到折叠起点。
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建")
    });
    let language_buffer =
        cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let (editor, cx) = cx.add_window_view({
        let language_buffer = language_buffer.clone();
        move |_, cx| Editor::from_language_buffer(language_buffer, EditorMode::Full, cx)
    });
    cx.run_until_parked();
    editor.update(cx, |editor, cx| editor.toggle_fold_at_line(Line::ZERO, cx));

    // 光标在折叠起点（anchor 行行尾，字节 11）。
    editor.update(cx, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(11)));
    });
    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));
    cx.dispatch_action(MoveRight);
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections().primary().head(),
            ByteOffset::new(27),
            "右箭头应一步跨过折叠，落在闭合括号"
        );
    });
    cx.dispatch_action(MoveLeft);
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections().primary().head(),
            ByteOffset::new(11),
            "左箭头应回到折叠起点"
        );
    });
}

#[gpui::test]
fn unfold_all_expands_every_fold(cx: &mut TestAppContext) {
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建")
    });
    let buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    cx.run_until_parked();

    // 手动折叠两个块体（各自隐藏 2 行，无占位行）：总行数 6 → 2。
    editor.update(cx, |editor, cx| editor.toggle_fold_at_line(Line::ZERO, cx));
    editor.update(cx, |editor, cx| {
        editor.toggle_fold_at_line(Line::new(3), cx)
    });
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        2
    );

    // 全部展开：恢复 6 行。
    editor.update(cx, |editor, cx| editor.unfold_all_ranges(cx));
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        6
    );
    assert!(!cx.read_entity(&editor, |editor, _| {
        editor
            .display_map
            .snapshot()
            .fold_anchor_lines()
            .contains(&Line::ZERO)
    }));
}
#[gpui::test]
fn diff_hunks_follow_buffer_edits_without_losing_highlight(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "line 0\nline 1\nline 2\n");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let editor = cx.new(|cx| Editor::for_language_buffer(buffer.clone(), cx));
    let source = buffer.clone();

    inject_editor_diff(
        &editor,
        &source,
        vec![DiffHunk {
            range: 1..2,
            old_range: 1..2,
            kind: DiffHunkKind::Modified,
        }],
        None,
        cx,
    );
    editor.update(cx, |editor, cx| {
        assert_eq!(editor.diff_hunks(cx).len(), 1, "注入后应立即可见");

        // 在 hunk 前插入一行后版本推进，已有 hunk 应跟随文本移动而不是消失。
        editor.multi_buffer.update(cx, |buffer, cx| {
            buffer
                .edit(
                    vec![Edit::insert(ByteOffset::ZERO, "changed\n").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .expect("测试编辑应成功");
        });
        assert_eq!(
            editor.diff_hunks(cx),
            &[DiffHunk {
                range: 2..3,
                old_range: 1..2,
                kind: DiffHunkKind::Modified,
            }],
            "编辑后 Git 高亮应跟随文本位置"
        );
    });
    // 后续 Git 刷新仍可用新的权威结果替换当前投影。
    inject_editor_diff(
        &editor,
        &source,
        vec![DiffHunk {
            range: 0..1,
            old_range: 0..0,
            kind: DiffHunkKind::Added,
        }],
        None,
        cx,
    );
    editor.update(cx, |editor, cx| {
        assert_eq!(editor.diff_hunks(cx).len(), 1, "重新注入后应恢复");
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

    let (line_count, continuation_offset) = cx.read_entity(&editor, |editor, _| {
        let line_count = editor.display_map.line_count();
        assert!(line_count > 1, "宽行应拆成多个显示行");
        let continuation = editor
            .display_map
            .display_point_to_offset(DisplayPoint::new(DisplayRow::new(1), DisplayColumn::ZERO))
            .expect("续行行首应可映射");
        (line_count, continuation)
    });

    // 点击第二个显示行（行高约 26px），光标应落在续行片段起点。
    // x 越过 gutter（约 60px）进入文本区，落在第二个显示行内。
    cx.simulate_click(point(px(80.), px(30.)), gpui::Modifiers::default());
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections().primary().head(),
            continuation_offset,
            "点击续行应把光标放到片段起点"
        );
    });
    assert!(line_count > 0);
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
    let source_end = {
        let buffer = engine_buffer(&source, cx);
        cx.read_entity(&buffer, |buffer, _| buffer.len_bytes())
    };
    let source_multi = source.clone();
    let combined = cx.new(MultiBuffer::empty);
    combined.update(cx, |combined, cx| {
        combined.set_excerpts(
            vec![MultiBufferExcerpt::new(
                source_multi,
                TextRange::new(ByteOffset::ZERO, source_end).expect("完整片段范围应有效"),
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
    let unwrapped_rows = cx.read_entity(&editor, |editor, _| editor.display_map.line_count());

    editor.update(cx, |editor, cx| {
        editor.set_soft_wrap_mode(Some(SoftWrap::EditorWidth), cx);
    });
    cx.run_until_parked();
    let wrapped_rows = cx.read_entity(&editor, |editor, _| editor.display_map.line_count());

    assert!(
        wrapped_rows > unwrapped_rows,
        "MultiBuffer 应经过与普通 Editor 相同的 WrapMap；{unwrapped_rows} -> {wrapped_rows}"
    );

    editor.update(cx, |editor, cx| {
        editor.toggle_buffer_fold(PathBuf::from("文档/引擎.md"), cx)
    });
    let folded_rows = cx.read_entity(&editor, |editor, _| editor.display_map.line_count());
    assert_eq!(folded_rows, 2, "整文件折叠后只保留两行高的 BufferHeader");

    editor.update(cx, |editor, cx| {
        editor.toggle_buffer_fold(PathBuf::from("文档/引擎.md"), cx)
    });
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        wrapped_rows,
        "再次点击 header chevron 应完整恢复 excerpts"
    );
}

#[gpui::test]
fn viewport_highlight_cache_reuses_identical_query_frames(cx: &mut TestAppContext) {
    // 光标闪烁/焦点切换等重复帧：相同版本与区间的高亮查询直接命中跨帧缓存，不重复执行树查询。
    let buffer = cx.new(|_| {
        Buffer::scratch(
            "fn main() { let value = 1; }\n".to_owned(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建")
    });
    let buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    cx.run_until_parked();

    let (first_ptr, second_ptr, first_len) = cx.read_entity(&editor, |editor, _| {
        let snapshot = editor.display_map.snapshot();
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, 1)
            .expect("视口应可读取");
        let first = snapshot.highlighted_spans_for_viewport(&viewport);
        let first_ptr = Arc::as_ptr(&first);
        let first_len = first.len();
        let second = snapshot.highlighted_spans_for_viewport(&viewport);
        (first_ptr, Arc::as_ptr(&second), first_len)
    });
    assert!(first_len > 0, "rust 源码视口应产出高亮 spans");
    assert_eq!(
        first_ptr, second_ptr,
        "相同帧的重复查询应命中缓存（同一 Arc）"
    );
}

#[gpui::test]
fn viewport_highlight_cache_invalidates_after_text_edit(cx: &mut TestAppContext) {
    let buffer = cx.new(|_| {
        Buffer::scratch(
            "fn main() { let value = 1; }\n".to_owned(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建")
    });
    let buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let engine = engine_buffer(&buffer, cx);
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    cx.run_until_parked();

    let first_ptr = cx.read_entity(&editor, |editor, _| {
        let snapshot = editor.display_map.snapshot();
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, 1)
            .expect("视口应可读取");
        Arc::as_ptr(&snapshot.highlighted_spans_for_viewport(&viewport))
    });

    // 编辑文本：版本推进后同一视口的查询结果必须重新计算。
    cx.update_entity(&engine, |buffer, cx| {
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(20), " // 注释").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("插入应成功");
        cx.notify();
    });
    cx.run_until_parked();

    let second_ptr = cx.read_entity(&editor, |editor, _| {
        let snapshot = editor.display_map.snapshot();
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, 1)
            .expect("视口应可读取");
        Arc::as_ptr(&snapshot.highlighted_spans_for_viewport(&viewport))
    });
    assert_ne!(first_ptr, second_ptr, "编辑后高亮必须重新查询");
}

#[gpui::test]
fn long_line_highlight_query_is_clipped_to_render_budget(cx: &mut TestAppContext) {
    // 超长单行：高亮查询只覆盖可见前缀（渲染端同样只塑形前 MAX_RENDERED_LINE_LEN 字节）。
    let long = "let text = \"".to_owned() + &"a".repeat(8192) + "\";\n";
    let buffer =
        cx.new(|_| Buffer::scratch(long, BufferConfig::default()).expect("测试 Buffer 应能创建"));
    let buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer.clone(), EditorMode::Full, cx));
    cx.run_until_parked();

    cx.read_entity(&editor, |editor, _| {
        let snapshot = editor.display_map.snapshot();
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, 1)
            .expect("视口应可读取");
        let spans = snapshot.highlighted_spans_for_viewport(&viewport);
        let buffer = editor.display_map.buffer_snapshot();
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
    let buffer = cx.new(|_| {
        Buffer::scratch(format!("{long}\n{long}\n"), BufferConfig::default())
            .expect("测试 Buffer 应能创建")
    });
    let buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    cx.run_until_parked();

    let (windowed_len, window_start, full_len) = cx.read_entity(&editor, |editor, app| {
        let snapshot = editor.display_map.snapshot();
        let viewport = snapshot
            .slice_viewport(DisplayRow::ZERO, 2)
            .expect("视口应可读取");
        let base = gpui::TextRun {
            len: 0,
            font: gpui::Font {
                family: ".SystemUIFont".into(),
                features: Default::default(),
                fallbacks: None,
                weight: gpui::FontWeight::default(),
                style: gpui::FontStyle::default(),
            },
            color: gpui::white(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let style = crate::display_map::RowStyleInput {
            visible_highlights: &[],
            highlight_styles: &[],
            search_backgrounds: &[],
            marked_ranges: &[],
        };
        let window = Some((200usize, 500usize));
        let row0 = crate::display_map::render_viewport_row(
            viewport.rows()[0].kind(),
            &snapshot,
            &style,
            base.clone(),
            window,
            app,
        );
        let row1 = crate::display_map::render_viewport_row(
            viewport.rows()[1].kind(),
            &snapshot,
            &style,
            base.clone(),
            None,
            app,
        );
        (
            row0.display_text.len(),
            row0.window_start_column,
            row1.display_text.len(),
        )
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
        combined.set_excerpts(
            vec![MultiBufferExcerpt::line_range(source_multi, 5..7, cx)],
            cx,
        );
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
        combined.set_excerpts(
            vec![
                MultiBufferExcerpt::line_range(work_multi.clone(), 0..1, cx),
                MultiBufferExcerpt::line_range(head_multi, 1..3, cx)
                    .with_diff_kind(ExcerptDiffKind::Deleted)
                    .with_editable(false),
                MultiBufferExcerpt::line_range(work_multi, 1..3, cx),
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

/// 回归：普通编辑器展开 Deleted hunk 后与多文件编辑器共用 MultiBuffer excerpts 机制，
/// 光标可停在 HEAD 旧行上并显示修订行列；折叠恢复整文件。
#[gpui::test]
fn plain_editor_expanding_deleted_hunk_uses_excerpts_and_allows_cursor(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\nb\nc");
    buffer.update(cx, |buffer, cx| {
        buffer.set_file_path(PathBuf::from("src/a.rs"), cx)
    });
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    // 普通编辑器的 diff 注入路径：hunks + HEAD 全文 + 展开删除块。
    let source = buffer.clone();
    inject_editor_diff(
        &editor,
        &source,
        vec![DiffHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
        }],
        Some(Arc::from("a\nold1\nold2\nc")),
        cx,
    );
    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    cx.run_until_parked();

    // 展开后：HEAD 旧行作为只读 excerpt 插入删除点；光标可停在 HEAD 旧行并显示修订行列。
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nold1\nold2\nb\nc");
        let snapshot = editor.display_map.snapshot();
        let rendering = hunk_rendering(
            &snapshot,
            editor.diff_hunks(cx),
            &editor.diff_hunk_expanded(cx),
            editor.diff_hunk_old_ranges(cx),
        );
        assert_eq!(
            rendering.hit_regions,
            vec![(1..3, 0, DiffHunkKind::Deleted)],
            "展开的纯删除块仍应暴露 gutter 折叠点击区"
        );
    });
    editor.update(cx, |editor, cx| editor.select_byte_range(2..2, cx));
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.cursor_text(cx), "2:1");
    });
    // 工作区段仍可编辑（"b" → "B"），写回工作区文件。
    editor.update(cx, |editor, cx| {
        editor.select_byte_range(12..13, cx);
        editor.replace_text(None, "B", cx);
    });
    assert_eq!(buffer_text(&buffer, cx), "a\nB\nc");

    // 折叠：恢复整文件（HEAD 旧行消失）。
    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nB\nc");
    });
}

/// 折叠的删除块三角锚点：删除第 17 行（1-based，0-based 16）后，锚点行必须是组合 0-based 16 行（16/17 行边界），不能是 15 或 17。
#[gpui::test]
fn folded_deleted_hunk_anchor_is_at_the_deletion_row_boundary(cx: &mut TestAppContext) {
    let buffer = test_buffer(
        cx,
        (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
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
        vec![DiffHunk {
            range: 16..16,
            old_range: 16..17,
            kind: DiffHunkKind::Deleted,
        }],
        Some(Arc::from(
            (1..=20)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )),
        cx,
    );
    cx.run_until_parked();

    cx.read_entity(&editor, |editor, cx| {
        // 折叠态：组合保持新侧 20 行（普通编辑器整文件模式）。
        assert_eq!(
            editor.text(cx),
            (1..=20)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let snapshot = editor.display_map.snapshot();
        let rendering = hunk_rendering(
            &snapshot,
            editor.diff_hunks(cx),
            &editor.diff_hunk_expanded(cx),
            editor.diff_hunk_old_ranges(cx),
        );
        eprintln!("DEBUG hit_regions={:?}", rendering.hit_regions);
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
        vec![DiffHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
        }],
        Some(Arc::from("a\nold1\nold2\nc")),
        cx,
    );
    editor.update(cx, |editor, cx| {
        editor.toggle_diff_hunk_at(0, cx);
        assert_eq!(editor.text(cx), "a\nold1\nold2\nb\nc");
    });
    inject_editor_diff(&editor, &source, Vec::new(), None, cx);
    editor.update(cx, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nb\nc");
        assert!(editor.diff_hunk_old_ranges(cx).is_empty());
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
        vec![DiffHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
        }],
        Some(Arc::from("a\nold1\nold2\nold3\nc")),
        cx,
    );
    editor.update(cx, |editor, cx| {
        editor.toggle_diff_hunk_at(0, cx);
        assert_eq!(editor.text(cx), "a\nold1\nold2\nb\nc");
    });

    // 编辑后 hunk 边界变化：旧侧范围 1..3 扩大为 1..4（与旧 hunk 重叠即可识别为同一 hunk）。
    inject_editor_diff(
        &editor,
        &source,
        vec![DiffHunk {
            range: 1..1,
            old_range: 1..4,
            kind: DiffHunkKind::Deleted,
        }],
        Some(Arc::from("a\nold1\nold2\nold3\nc")),
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
        vec![
            DiffHunk {
                range: 1..1,
                old_range: 1..2,
                kind: DiffHunkKind::Deleted,
            },
            DiffHunk {
                range: 3..3,
                old_range: 3..4,
                kind: DiffHunkKind::Deleted,
            },
        ],
        Some(Arc::from("a\nold1\nb\nold3\ne")),
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
        vec![DiffHunk {
            range: 3..3,
            old_range: 3..4,
            kind: DiffHunkKind::Deleted,
        }],
        Some(Arc::from("a\nold1\nb\nold3\ne")),
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
        vec![DiffHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
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
        vec![DiffHunk {
            range: 1..1,
            old_range: 1..4,
            kind: DiffHunkKind::Deleted,
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
        vec![DiffHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
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
        vec![DiffHunk {
            range: 1..2,
            old_range: 1..2,
            kind: DiffHunkKind::Modified,
        }],
        Some(Arc::from("a\nold\nc")),
        cx,
    );
    editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));

    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "a\nold\nnew\nc");
        let snapshot = editor.display_map.snapshot();
        let rendering = hunk_rendering(
            &snapshot,
            editor.diff_hunks(cx),
            &editor.diff_hunk_expanded(cx),
            editor.diff_hunk_old_ranges(cx),
        );
        assert_eq!(
            rendering.diff_rows,
            vec![(1..2, DiffHunkKind::Deleted), (2..3, DiffHunkKind::Added),],
            "展开的普通编辑器修改块应保留旧侧红色行和新侧绿色行"
        );
        assert_eq!(
            rendering.strips,
            vec![(1..3, DiffHunkKind::Modified)],
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
        vec![DiffHunk {
            range: 1..1,
            old_range: 1..3,
            kind: DiffHunkKind::Deleted,
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
