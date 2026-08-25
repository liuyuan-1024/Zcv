use gpui::{TestAppContext, point, px};
use std::path::PathBuf;
use zcv_multi_buffer::MultiBufferExcerpt;
use zcv_text::{ByteOffset, Edit, TextRange, TransactionMetadata};

use super::common::{engine_buffer, test_buffer};
use super::*;

#[gpui::test]
fn deleted_hunk_expands_and_collapses_inserted_lines(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\nb\nc");
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx));
    cx.run_until_parked();
    let base_rows = cx.read_entity(&editor, |editor, _| editor.display_map.line_count());
    assert_eq!(base_rows, 3);

    // 注入 Deleted hunk（新侧行 1 处删除了 HEAD 的 1..3 行）+ HEAD 全文。
    editor.update(cx, |editor, cx| {
        editor.set_diff_hunks(
            vec![DiffHunk {
                range: 1..1,
                old_range: 1..3,
                kind: DiffHunkKind::Deleted,
            }],
            cx,
        );
        editor.set_deleted_hunk_text(Some(Arc::from("a\nold1\nold2\nc")), cx);
    });
    // 未展开：行数不变。
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        3
    );

    // 展开删除块：HEAD 的 1..3 行（old1/old2）作为合成行插入。
    editor.update(cx, |editor, cx| editor.toggle_deleted_hunk(1..3, cx));
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        5,
        "展开后应增加 2 个被删除行"
    );

    // 再折叠：回到 3 行。
    editor.update(cx, |editor, cx| editor.toggle_deleted_hunk(1..3, cx));
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        3,
        "折叠后应回到 3 行"
    );
}
#[gpui::test]
fn toggle_fold_collapses_and_expands_the_cursor_block(cx: &mut TestAppContext) {
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建")
    });
    let buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx));
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
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx));
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
    let crate::display_map::WrapViewportRowKind::Text { text, .. } = viewport.rows()[0].kind();
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
        super::super::display_map::ProjectedPoint::new(
            super::super::display_map::ProjectedLineIndex::new(0),
            zcv_text::LogicalColumn::new(12)
        )
    );
    assert_eq!(
        projected[0].end(),
        super::super::display_map::ProjectedPoint::new(
            super::super::display_map::ProjectedLineIndex::new(0),
            zcv_text::LogicalColumn::new(13)
        )
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
    let editor = cx.new(|cx| Editor::from_language_buffer(buffer, EditorMode::Full, cx));
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
fn diff_hunks_are_gated_by_buffer_version(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "line 0\nline 1\nline 2\n");
    let editor = cx.new(|cx| Editor::for_language_buffer(buffer.clone(), cx));

    editor.update(cx, |editor, cx| {
        editor.set_diff_hunks(
            vec![DiffHunk {
                range: 1..2,
                old_range: 1..2,
                kind: DiffHunkKind::Modified,
            }],
            cx,
        );
        assert_eq!(editor.diff_hunks(cx).len(), 1, "注入后应立即可见");

        // 编辑 buffer 后版本推进，行号已失配，hunks 应被门控为不可见。
        editor.set_text("changed\nline 1\nline 2\n", cx);
        assert!(
            editor.diff_hunks(cx).is_empty(),
            "编辑后行号失配，应隐藏 hunks 等待重新注入"
        );

        // 重新注入（新版本）后恢复可见。
        editor.set_diff_hunks(
            vec![DiffHunk {
                range: 0..1,
                old_range: 0..0,
                kind: DiffHunkKind::Added,
            }],
            cx,
        );
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
    let source_multi = cx.new({
        let source = source.clone();
        move |cx| MultiBuffer::singleton(source, cx)
    });
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
