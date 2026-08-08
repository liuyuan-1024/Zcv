use super::common::test_buffer;
use super::*;
use gpui::{TestAppContext, point, px};
use zcv_engine::ByteOffset;

#[gpui::test]
fn deleted_hunk_expands_and_collapses_inserted_lines(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a\nb\nc");
    let editor = cx.new(|cx| Editor::new(buffer, EditorMode::Full, cx));
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
fn inlays_project_through_editor_display_pipeline(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "ab\ncd\n");
    let editor = cx.new(|cx| Editor::new(buffer, EditorMode::Full, cx));
    cx.run_until_parked();
    editor.update(cx, |editor, cx| {
        editor.set_inlays(
            vec![Inlay {
                position: ByteOffset::new(1),
                text: ": hint".to_owned(),
            }],
            cx,
        );
    });
    // 行内提示不占行数（"ab\ncd\n" = 3 行）。
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        3
    );
    // 经消费链投影：视口行文本含注入文本。
    let snapshot = cx.read_entity(&editor, |editor, _| editor.display_map.snapshot());
    let viewport = snapshot
        .slice_viewport(DisplayRow::ZERO, 1)
        .expect("视口应可读取");
    let crate::display_map::WrapViewportRowKind::Text { text, .. } = viewport.rows()[0].kind();
    assert_eq!(text.as_ref(), "a: hintb\n");
}
#[gpui::test]
fn toggle_fold_collapses_and_expands_the_cursor_block(cx: &mut TestAppContext) {
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建")
    });
    let buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let editor = cx.new(|cx| Editor::new(buffer, EditorMode::Full, cx));
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
    eprintln!("fold_ranges: {:?}", fold_ranges);

    // 折叠 fn main（入口行 0）：隐藏块内 2 行，无占位行，总行数 6 → 4。
    editor.update(cx, |editor, cx| editor.toggle_fold_at_line(Line::ZERO, cx));
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        4
    );
    assert!(cx.read_entity(&editor, |editor, _| {
        editor.display_map.snapshot().is_line_folded(Line::ZERO)
    }));

    // 再次切换：展开，恢复 6 行。
    editor.update(cx, |editor, cx| editor.toggle_fold_at_line(Line::ZERO, cx));
    assert_eq!(
        cx.read_entity(&editor, |editor, _| editor.display_map.line_count()),
        6
    );
    assert!(!cx.read_entity(&editor, |editor, _| {
        editor.display_map.snapshot().is_line_folded(Line::ZERO)
    }));
}

#[gpui::test]
fn fold_ranges_survive_edits_and_folded_state_follows(cx: &mut TestAppContext) {
    // 回归：编辑后折叠范围与折叠状态必须保持（crease 箭头显示依赖 fold_ranges / is_line_folded）。
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建")
    });
    let language_buffer =
        cx.new(|cx| LanguageBuffer::new(buffer.clone(), Some(PathBuf::from("main.rs")), cx));
    let editor = cx.new(|cx| Editor::new(language_buffer, EditorMode::Full, cx));
    cx.run_until_parked();

    // 编辑 buffer：在首行后插入一行注释。
    cx.update_entity(&buffer, |buffer, cx| {
        buffer
            .insert(ByteOffset::new(7), "// 注释\n")
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
        editor.display_map.snapshot().is_line_folded(Line::new(1))
    }));
}
#[gpui::test]
fn unfold_all_expands_every_fold(cx: &mut TestAppContext) {
    let text = "fn main() {\n    let x = 1;\n}\nfn other() {\n    let y = 2;\n}";
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建")
    });
    let buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("main.rs")), cx));
    let editor = cx.new(|cx| Editor::new(buffer, EditorMode::Full, cx));
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
        editor.display_map.snapshot().is_line_folded(Line::ZERO)
    }));
}
#[gpui::test]
fn diff_hunks_are_gated_by_buffer_version(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "line 0\nline 1\nline 2\n");
    let editor = cx.new(|cx| Editor::for_buffer(buffer.clone(), cx));

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
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    editor.update(cx, |editor, cx| {
        editor.set_soft_wrap(SoftWrap::EditorWidth, 80, cx);
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
