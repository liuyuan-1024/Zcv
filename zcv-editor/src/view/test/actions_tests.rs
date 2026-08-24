use gpui::{TestAppContext, point, px, size};
use zcv_text::{ByteOffset, Edit, TransactionId, TransactionMetadata};

use super::common::{buffer_text, engine_buffer, test_buffer};
use super::*;
use crate::display_map::{DisplayPoint, DisplayRow};
use crate::{Selection, SelectionSet};

#[gpui::test]
fn editors_share_buffer_but_keep_view_state_independent(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "abc");
    let first = cx.new(|cx| Editor::for_language_buffer(buffer.clone(), cx));
    let second = cx.new(|cx| Editor::for_language_buffer(buffer.clone(), cx));

    cx.update_entity(&first, |editor, cx| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(1)));
        editor
            .scroll_manager
            .update_viewport(1, px(100.0), px(40.0), px(200.0), px(20.0));
        editor.scroll_manager.scroll_by(point(px(-4.0), px(0.0)));
        editor
            .selection_history
            .insert_transaction(TransactionId::new(1), SelectionSet::caret(ByteOffset::ZERO));
        let selections = editor.selections().clone();
        editor
            .selection_history
            .transaction_mut(TransactionId::new(1))
            .expect("插入后应存在")
            .set_redo(selections);
        editor.singleton_buffer(cx).update(cx, |buffer, cx| {
            buffer
                .edit(
                    [Edit::insert(ByteOffset::new(3), "d").unwrap()],
                    TransactionMetadata::default(),
                )
                .expect("共享 Buffer 编辑应成功");
            cx.notify();
        });
    });

    cx.read_entity(&second, |editor, cx| {
        assert_eq!(editor.mode, EditorMode::Full);
        assert_eq!(editor.singleton_buffer(cx), buffer.read(cx).buffer());
        assert_eq!(
            editor.singleton_buffer(cx).read(cx).len_bytes(),
            ByteOffset::new(4)
        );
        assert_eq!(editor.render_snapshot().len_bytes(), ByteOffset::new(4));
        assert_eq!(editor.selections(), SelectionSet::caret(ByteOffset::ZERO));
        assert_eq!(editor.scroll_manager.anchor(), DisplayPoint::ZERO);
        assert_eq!(editor.scroll_manager.offset(), point(px(0.0), px(0.0)));
        assert!(
            editor
                .selection_history
                .transaction(TransactionId::new(1))
                .is_none()
        );
    });

    cx.read_entity(&first, |editor, _| {
        assert_eq!(editor.scroll_manager.offset().x, px(4.0));
        let history = editor
            .selection_history
            .transaction(TransactionId::new(1))
            .expect("第一个 Editor 应保存自己的选区历史");
        assert_eq!(history.undo(), &SelectionSet::caret(ByteOffset::ZERO));
        assert_eq!(
            history.redo(),
            Some(&SelectionSet::caret(ByteOffset::new(1)))
        );
    });
}
#[gpui::test]
fn other_editor_editing_shared_buffer_moves_this_editors_selection(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "abc");
    let first = cx.new(|cx| Editor::for_language_buffer(buffer.clone(), cx));
    let second = cx.new(|cx| Editor::for_language_buffer(buffer.clone(), cx));

    // 两个 Editor 的光标都在偏移 3（"abc" 末尾）。
    cx.update_entity(&first, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(3)));
    });
    cx.update_entity(&second, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(3)));
    });

    // 第一个 Editor 在光标处输入 "d"。
    cx.update_entity(&first, |editor, cx| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(3)));
        editor.replace_text(None, "d", cx);
    });
    cx.run_until_parked();

    // 第二个 Editor 的选区端点锚点自动跟随到新文本之后。
    cx.read_entity(&second, |editor, _| {
        assert_eq!(editor.selections().primary().head(), ByteOffset::new(4));
    });
    cx.read_entity(&first, |editor, _| {
        assert_eq!(editor.selections().primary().head(), ByteOffset::new(4));
    });
}
#[gpui::test]
fn external_reload_moves_selection_through_diff(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "alpha\nbravo\ncharlie");
    let editor = cx.new(|cx| Editor::for_language_buffer(buffer.clone(), cx));
    // 光标在 "bravo" 行内 "br" 之后（行内第 2 字节）。
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(8)));
    });

    // 外部在行内插入 "x"：diff patch 保留 "br" 与 "avo" 匹配段，端点映射到插入 "x" 之后。
    let raw_buffer = engine_buffer(&buffer, cx);
    cx.update_entity(&raw_buffer, |buffer, cx| {
        buffer
            .reload_from_text("alpha\nbrxavo\ncharlie".to_owned())
            .expect("外部 reload 应成功");
        cx.notify();
    });
    cx.run_until_parked();

    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections().primary().head(), ByteOffset::new(9));
    });
}
#[gpui::test]
fn external_reload_collapses_selection_when_text_is_rewritten(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "abc");
    let editor = cx.new(|cx| Editor::for_language_buffer(buffer.clone(), cx));
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(2)));
    });

    // 完全重写（无公共内容）：diff 回退为整体替换段，光标塌缩到文档开头。
    let raw_buffer = engine_buffer(&buffer, cx);
    cx.update_entity(&raw_buffer, |buffer, cx| {
        buffer
            .reload_from_text("x".to_owned())
            .expect("外部 reload 应成功");
        cx.notify();
    });
    cx.run_until_parked();

    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), SelectionSet::default());
    });
}
#[gpui::test]
fn constructors_create_expected_modes_and_independent_scratch_buffers(cx: &mut TestAppContext) {
    let single_line = cx.new(Editor::single_line);
    let auto_height = cx.new(|cx| Editor::auto_height(2, Some(6), cx));

    let single_buffer = cx.read_entity(&single_line, |editor, cx| {
        assert_eq!(editor.mode, EditorMode::SingleLine);
        assert_eq!(editor.selections(), SelectionSet::default());
        assert_eq!(
            editor.display_map.version(),
            editor.singleton_buffer(cx).read(cx).version()
        );
        let _focus = editor.focus_handle();
        editor.singleton_buffer(cx)
    });
    let auto_height_buffer = cx.read_entity(&auto_height, |editor, cx| {
        assert_eq!(
            editor.mode,
            EditorMode::AutoHeight {
                min_lines: 2,
                max_lines: Some(6),
            }
        );
        editor.singleton_buffer(cx)
    });

    assert_ne!(single_buffer, auto_height_buffer);
}
#[gpui::test]
fn editor_element_renders_multiline_unicode_text(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a你\n😀b");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.run_until_parked();
    cx.simulate_click(point(px(1000.), px(12.)), gpui::Modifiers::default());
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.render_snapshot().line_count(), 2);
        assert_eq!(editor.selections().primary().head(), ByteOffset::new(4));
    });
}
#[gpui::test]
fn clicking_the_gutter_selects_a_logical_line(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "first\nsecond\nthird");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.run_until_parked();
    cx.simulate_click(point(px(4.), px(32.)), gpui::Modifiers::default());

    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections(),
            SelectionSet::new(vec![Selection::new(
                ByteOffset::new(6),
                ByteOffset::new(13)
            )])
        );
    });

    cx.simulate_click(
        point(px(4.), px(58.)),
        gpui::Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections(),
            SelectionSet::new(vec![Selection::new(
                ByteOffset::new(6),
                ByteOffset::new(18)
            )])
        );
    });
}
#[gpui::test]
fn committed_input_uses_element_input_handler_and_preserves_unicode(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.simulate_click(point(px(4.), px(12.)), gpui::Modifiers::default());
    cx.simulate_input("中😀e\u{301}");

    assert_eq!(buffer_text(&buffer, cx), "中😀e\u{301}");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections().primary().head(),
            ByteOffset::new("中😀e\u{301}".len())
        );
        assert!(editor.composition.is_none());
    });
}
#[gpui::test]
fn editor_actions_move_extend_delete_and_restore_unicode_selection(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a😀b");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));
    cx.dispatch_action(MoveRight);
    cx.dispatch_action(SelectRight);
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections(),
            SelectionSet::new(vec![Selection::new(ByteOffset::new(1), ByteOffset::new(5))])
        );
    });

    cx.dispatch_action(Backspace);
    assert_eq!(buffer_text(&buffer, cx), "ab");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), SelectionSet::caret(ByteOffset::new(1)));
    });

    cx.dispatch_action(Undo);
    assert_eq!(buffer_text(&buffer, cx), "a😀b");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections(),
            SelectionSet::new(vec![Selection::new(ByteOffset::new(1), ByteOffset::new(5))])
        );
    });

    cx.dispatch_action(Redo);
    assert_eq!(buffer_text(&buffer, cx), "ab");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), SelectionSet::caret(ByteOffset::new(1)));
    });
}

#[gpui::test]
fn deleting_a_reversed_selection_always_leaves_a_caret_at_its_start(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "abcdef");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::new(vec![Selection::new(
            ByteOffset::new(5),
            ByteOffset::new(2),
        )]));
    });
    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));
    cx.dispatch_action(Backspace);

    assert_eq!(buffer_text(&buffer, cx), "abf");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), SelectionSet::caret(ByteOffset::new(2)));
    });
}

#[gpui::test]
fn replacing_a_reversed_selection_places_the_caret_after_inserted_text(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "abcdef");
    let editor = cx.new(|cx| Editor::for_language_buffer(buffer.clone(), cx));

    cx.update_entity(&editor, |editor, cx| {
        editor.set_selections(SelectionSet::new(vec![Selection::new(
            ByteOffset::new(5),
            ByteOffset::new(2),
        )]));
        editor.replace_text(None, "XYZ", cx);
    });

    assert_eq!(buffer_text(&buffer, cx), "abXYZf");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), SelectionSet::caret(ByteOffset::new(5)));
    });
}
#[gpui::test]
fn expand_selection_uses_tree_sitter_ancestors(cx: &mut TestAppContext) {
    let source = "fn main() { let value = 1; }\n";
    let raw_buffer = cx.new(|_| {
        Buffer::scratch(source.to_owned(), BufferConfig::default())
            .expect("Rust 测试 Buffer 应能创建")
    });
    let language_buffer = cx.new({
        let raw_buffer = raw_buffer.clone();
        move |cx| LanguageBuffer::new(raw_buffer, Some(PathBuf::from("main.rs")), cx)
    });
    cx.run_until_parked();
    let (editor, cx) = cx.add_window_view({
        let language_buffer = language_buffer.clone();
        move |_, cx| Editor::for_language_buffer(language_buffer, cx)
    });
    cx.run_until_parked();
    let value = source.find("value").unwrap();
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(value)));
    });
    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

    cx.dispatch_action(ExpandSelection);
    cx.read_entity(&editor, |editor, _| {
        let selection = editor.selections().primary().range();
        assert_eq!(
            &source[selection.start().get()..selection.end().get()],
            "value"
        );
    });
    cx.dispatch_action(ExpandSelection);
    cx.read_entity(&editor, |editor, _| {
        assert!(editor.selections().primary().range().len() > "value".len());
    });
}
#[gpui::test]
fn matching_brackets_come_from_tree_sitter_query(cx: &mut TestAppContext) {
    let source = "fn main() { call(); }\n";
    let raw_buffer = cx.new(|_| {
        Buffer::scratch(source.to_owned(), BufferConfig::default())
            .expect("Rust 测试 Buffer 应能创建")
    });
    let language_buffer = cx.new({
        let raw_buffer = raw_buffer.clone();
        move |cx| LanguageBuffer::new(raw_buffer, Some(PathBuf::from("main.rs")), cx)
    });
    let editor = cx.new({
        let language_buffer = language_buffer.clone();
        move |cx| Editor::for_language_buffer(language_buffer, cx)
    });
    cx.run_until_parked();
    let open = source.find("()").unwrap();
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(open + 1)));
    });

    cx.update_entity(&editor, |editor, _| {
        let pair = editor
            .matching_bracket_pair()
            .expect("光标旁的括号应由 tree-sitter query 匹配");
        assert_eq!(&source[pair.open], "(");
        assert_eq!(&source[pair.close], ")");
    });
}
#[gpui::test]
fn word_and_line_delete_actions_follow_editor_boundaries(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "alpha beta gamma");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(10)));
    });
    cx.dispatch_action(DeleteToPreviousWordStart);
    assert_eq!(buffer_text(&buffer, cx), "alpha  gamma");

    cx.update_entity(&editor, |editor, cx| {
        editor.set_text("alpha beta gamma", cx);
        editor.set_selections(SelectionSet::caret(ByteOffset::new(6)));
    });
    cx.dispatch_action(DeleteToNextWordEnd);
    assert_eq!(buffer_text(&buffer, cx), "alpha  gamma");

    cx.update_entity(&editor, |editor, cx| {
        editor.set_text("one two three four", cx);
        editor.set_selections(SelectionSet::new(vec![Selection::new(
            ByteOffset::new(4),
            ByteOffset::new(13),
        )]));
    });
    cx.dispatch_action(DeleteToBeginningOfLine);
    assert_eq!(buffer_text(&buffer, cx), " four");

    cx.update_entity(&editor, |editor, cx| {
        editor.set_text("one two three four", cx);
        editor.set_selections(SelectionSet::new(vec![Selection::new(
            ByteOffset::new(4),
            ByteOffset::new(13),
        )]));
    });
    cx.dispatch_action(DeleteToEndOfLine);
    assert_eq!(buffer_text(&buffer, cx), "one ");

    cx.update_entity(&editor, |editor, cx| {
        editor.set_text("one\ntwo", cx);
        editor.set_selections(SelectionSet::caret(ByteOffset::new(4)));
    });
    cx.dispatch_action(DeleteToBeginningOfLine);
    assert_eq!(buffer_text(&buffer, cx), "onetwo");
}
#[gpui::test]
fn document_boundary_actions_move_and_extend_selection(cx: &mut TestAppContext) {
    let text = "ab\n中😀z";
    let buffer = test_buffer(cx, text);
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    let end = ByteOffset::new(text.len());

    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));
    cx.dispatch_action(MoveToEnd);
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), SelectionSet::caret(end));
    });

    cx.dispatch_action(MoveToBeginning);
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), SelectionSet::caret(ByteOffset::ZERO));
    });

    let anchor = ByteOffset::new(2);
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(anchor));
    });
    cx.dispatch_action(SelectToEnd);
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections(),
            SelectionSet::new(vec![Selection::new(anchor, end)])
        );
    });

    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(anchor));
    });
    cx.dispatch_action(SelectToBeginning);
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections(),
            SelectionSet::new(vec![Selection::new(anchor, ByteOffset::ZERO)])
        );
    });
}
#[gpui::test]
fn page_actions_move_selection_and_viewport_together(cx: &mut TestAppContext) {
    let text = (0..40).map(|row| format!("{row}\n")).collect::<String>();
    let buffer = test_buffer(cx, text);
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.simulate_resize(size(px(100.), px(100.)));
    cx.run_until_parked();
    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

    let page_rows = cx.read_entity(&editor, |editor, _| {
        editor
            .scroll_manager
            .page_row_count()
            .expect("完成布局后应有可见页行数")
    });
    assert!(page_rows > 0);

    cx.dispatch_action(MovePageDown);
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, _| {
        let caret_row = editor
            .render_snapshot()
            .byte_to_position(editor.selections().primary().head())
            .expect("翻页后的光标应有效")
            .line()
            .get();
        assert_eq!(caret_row, page_rows);
        assert_eq!(
            editor.scroll_manager.anchor().row(),
            DisplayRow::new(page_rows)
        );
    });

    let raw_buffer = engine_buffer(&buffer, cx);
    let snapshot = cx.read_entity(&raw_buffer, |buffer, _| buffer.snapshot());
    let first_page = snapshot
        .line_start_byte(Line::new(page_rows))
        .expect("第一页目标行应存在");
    let second_page = snapshot
        .line_start_byte(Line::new(page_rows * 2))
        .expect("第二页目标行应存在");

    cx.dispatch_action(SelectPageDown);
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, _| {
        // 垂直移动持久保留目标列（从列 0 起始，目标列仍为 0）。
        assert_eq!(
            editor.selections(),
            SelectionSet::new(vec![
                Selection::new(first_page, second_page).with_goal(Some(0))
            ])
        );
        assert_eq!(
            editor.scroll_manager.anchor().row(),
            DisplayRow::new(page_rows * 2)
        );
    });

    cx.dispatch_action(MovePageUp);
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections(),
            SelectionSet::new(vec![Selection::caret(first_page).with_goal(Some(0))])
        );
        assert_eq!(
            editor.scroll_manager.anchor().row(),
            DisplayRow::new(page_rows)
        );
    });

    cx.dispatch_action(SelectPageUp);
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections(),
            SelectionSet::new(vec![
                Selection::new(first_page, ByteOffset::ZERO).with_goal(Some(0))
            ])
        );
        assert_eq!(editor.scroll_manager.anchor().row(), DisplayRow::ZERO);
    });
}
#[gpui::test]
fn clipboard_actions_edit_selected_text_through_transactions(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "hello");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::new(vec![Selection::new(
            ByteOffset::new(1),
            ByteOffset::new(4),
        )]));
    });
    cx.dispatch_action(Copy);
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("ell".to_owned())
        );
    });

    cx.dispatch_action(Cut);
    assert_eq!(buffer_text(&buffer, cx), "ho");
    cx.dispatch_action(Undo);
    assert_eq!(buffer_text(&buffer, cx), "hello");

    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(5)));
    });
    cx.dispatch_action(Paste);
    assert_eq!(buffer_text(&buffer, cx), "helloell");
    let raw_buffer = engine_buffer(&buffer, cx);
    assert!(cx.read_entity(&raw_buffer, |buffer, _| buffer.can_undo()));
}
#[gpui::test]
fn move_line_up_and_down_reorders_lines_and_follows_selection(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "alpha\nbravo\ncharlie\ndelta");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

    // 光标在第二行上移：整行移动，光标保持行内相对位置
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(9)));
    });
    cx.dispatch_action(MoveLineUp);
    assert_eq!(buffer_text(&buffer, cx), "bravo\nalpha\ncharlie\ndelta");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections().as_slice()[0].head(), ByteOffset::new(3));
    });

    // 光标在第二行下移：与下一行交换
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(8)));
    });
    cx.dispatch_action(MoveLineDown);
    assert_eq!(buffer_text(&buffer, cx), "bravo\ncharlie\nalpha\ndelta");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections().as_slice()[0].head(),
            ByteOffset::new(16)
        );
    });

    // 撤销恢复
    cx.dispatch_action(Undo);
    assert_eq!(buffer_text(&buffer, cx), "bravo\nalpha\ncharlie\ndelta");
}
#[gpui::test]
fn move_line_skips_document_edges_and_moves_multi_line_selection(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "alpha\nbravo\ncharlie\ndelta");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

    // 首行不能上移：文本不变
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(2)));
    });
    cx.dispatch_action(MoveLineUp);
    assert_eq!(buffer_text(&buffer, cx), "alpha\nbravo\ncharlie\ndelta");

    // 末行不能下移：文本不变
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(22)));
    });
    cx.dispatch_action(MoveLineDown);
    assert_eq!(buffer_text(&buffer, cx), "alpha\nbravo\ncharlie\ndelta");

    // 多行选区（bravo + charlie 两行）整体上移
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::new(vec![Selection::new(
            ByteOffset::new(6),
            ByteOffset::new(19),
        )]));
    });
    cx.dispatch_action(MoveLineUp);
    assert_eq!(buffer_text(&buffer, cx), "bravo\ncharlie\nalpha\ndelta");
}
#[gpui::test]
fn move_line_keeps_newline_separation_at_document_edge(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "alpha\nbravo\ncharlie\ndelta");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

    // 倒数第二行下移到末行：行块与无换行的末行交换，换行必须保持
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(15))); // charlie 行
    });
    cx.dispatch_action(MoveLineDown);
    assert_eq!(buffer_text(&buffer, cx), "alpha\nbravo\ndelta\ncharlie");

    // 末行上移到倒数第二行
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(20))); // charlie（末行）
    });
    cx.dispatch_action(MoveLineUp);
    assert_eq!(buffer_text(&buffer, cx), "alpha\nbravo\ncharlie\ndelta");

    // 从首行连续下移三次，行块沉到文档末尾，光标始终跟随
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::caret(ByteOffset::new(2))); // alpha 行
    });
    for _ in 0..3 {
        cx.dispatch_action(MoveLineDown);
    }
    assert_eq!(buffer_text(&buffer, cx), "bravo\ncharlie\ndelta\nalpha");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections().as_slice()[0].head(),
            ByteOffset::new(22)
        );
    });
}
#[gpui::test]
fn move_line_moves_rows_of_partial_selection_and_keeps_shape(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "alpha\nbravo\ncharlie\ndelta");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

    // 选中 bravo 行内部分文本（非整行选区）上移：所在行块移动，选区形状保持
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::new(vec![Selection::new(
            ByteOffset::new(7),
            ByteOffset::new(9),
        )]));
    });
    cx.dispatch_action(MoveLineUp);
    assert_eq!(buffer_text(&buffer, cx), "bravo\nalpha\ncharlie\ndelta");
    cx.read_entity(&editor, |editor, _| {
        let selection = editor.selections().as_slice()[0];
        assert_eq!(selection.start(), ByteOffset::new(1));
        assert_eq!(selection.end(), ByteOffset::new(3));
    });

    // 跨行选区（alpha 行首到 charlie 行内）下移：两个整行块移动，选区形状保持
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::new(vec![Selection::new(
            ByteOffset::new(6),
            ByteOffset::new(17),
        )]));
    });
    cx.dispatch_action(MoveLineDown);
    assert_eq!(buffer_text(&buffer, cx), "bravo\ndelta\nalpha\ncharlie");
    cx.read_entity(&editor, |editor, _| {
        let selection = editor.selections().as_slice()[0];
        assert_eq!(selection.start(), ByteOffset::new(12));
        assert_eq!(selection.end(), ByteOffset::new(23));
    });
}
#[gpui::test]
fn directional_moves_collapse_selection_to_its_edges(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "alpha\nbravo\ncharlie\ndelta");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

    // 选区按 ←：光标折叠到选区左端（不移动）
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::new(vec![Selection::new(
            ByteOffset::new(7),
            ByteOffset::new(11),
        )]));
    });
    cx.dispatch_action(MoveLeft);
    cx.read_entity(&editor, |editor, _| {
        let selection = editor.selections().as_slice()[0];
        assert!(selection.is_caret());
        assert_eq!(selection.head(), ByteOffset::new(7));
    });

    // 选区按 →：光标折叠到选区右端
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::new(vec![Selection::new(
            ByteOffset::new(7),
            ByteOffset::new(11),
        )]));
    });
    cx.dispatch_action(MoveRight);
    cx.read_entity(&editor, |editor, _| {
        let selection = editor.selections().as_slice()[0];
        assert!(selection.is_caret());
        assert_eq!(selection.head(), ByteOffset::new(11));
    });

    // 跨行选区按 ↑：光标从选区顶端出发向上移动一行
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::new(vec![Selection::new(
            ByteOffset::new(7),
            ByteOffset::new(18),
        )]));
    });
    cx.dispatch_action(MoveUp);
    cx.read_entity(&editor, |editor, _| {
        let selection = editor.selections().as_slice()[0];
        assert!(selection.is_caret());
        assert_eq!(selection.head(), ByteOffset::new(1));
    });

    // 跨行选区按 ↓：光标从选区底端出发向下移动一行（列越界钳制到行尾）
    cx.update_entity(&editor, |editor, _| {
        editor.set_selections(SelectionSet::new(vec![Selection::new(
            ByteOffset::new(7),
            ByteOffset::new(18),
        )]));
    });
    cx.dispatch_action(MoveDown);
    cx.read_entity(&editor, |editor, _| {
        let selection = editor.selections().as_slice()[0];
        assert!(selection.is_caret());
        // 列 6 越界钳制到末行行尾（delta 无换行，行尾即文档末尾）。
        assert_eq!(selection.head(), ByteOffset::new(25));
    });
}
#[gpui::test]
fn word_line_and_vertical_movement_use_engine_boundaries(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "alpha 你好\nxy");
    let editor = cx.new({
        let buffer = buffer.clone();
        move |cx| {
            let mut editor = Editor::for_language_buffer(buffer, cx);
            editor.set_selections(SelectionSet::caret(ByteOffset::new("alpha 你好".len())));
            editor
        }
    });

    cx.update_entity(&editor, |editor, cx| {
        editor.move_selections(MovementDirection::Previous, MovementUnit::Word, false, cx);
        assert_eq!(editor.selections().primary().head(), ByteOffset::new(6));

        editor.move_selections(MovementDirection::Next, MovementUnit::LineEdge, false, cx);
        assert_eq!(
            editor.selections().primary().head(),
            ByteOffset::new("alpha 你好".len())
        );

        editor.move_selections(MovementDirection::Next, Motion::LineStep, false, cx);
        assert_eq!(
            editor.selections().primary().head(),
            ByteOffset::new("alpha 你好\nxy".len())
        );
    });
}
#[gpui::test]
fn newline_is_a_transaction_and_undo_restores_selection(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "ab");
    let editor = cx.new({
        let buffer = buffer.clone();
        move |cx| {
            let mut editor = Editor::for_language_buffer(buffer, cx);
            editor.set_selections(SelectionSet::caret(ByteOffset::new(1)));
            editor
        }
    });

    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    assert_eq!(buffer_text(&buffer, cx), "a\nb");
    let raw_buffer = engine_buffer(&buffer, cx);
    assert!(cx.read_entity(&raw_buffer, |buffer, _| buffer.can_undo()));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), SelectionSet::caret(ByteOffset::new(2)));
    });

    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), "ab");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), SelectionSet::caret(ByteOffset::new(1)));
    });
}
#[gpui::test]
fn newline_uses_tree_sitter_indent_query(cx: &mut TestAppContext) {
    let source = "fn main() {}\n";
    let caret = source.find('{').unwrap() + 1;
    let raw_buffer = cx.new(|_| {
        Buffer::scratch(source.to_owned(), BufferConfig::default())
            .expect("Rust 测试 Buffer 应能创建")
    });
    let language_buffer = cx.new({
        let raw_buffer = raw_buffer.clone();
        move |cx| LanguageBuffer::new(raw_buffer, Some(PathBuf::from("main.rs")), cx)
    });
    let editor = cx.new({
        let language_buffer = language_buffer.clone();
        move |cx| {
            let mut editor = Editor::for_language_buffer(language_buffer, cx);
            editor.set_selections(SelectionSet::caret(ByteOffset::new(caret)));
            editor
        }
    });
    cx.run_until_parked();

    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    // 光标在 `{` 与自动补全的 `}` 之间：额外补一个基准缩进空行（newline 配对行为）。
    assert_eq!(buffer_text(&language_buffer, cx), "fn main() {\n    \n}\n");
}

#[gpui::test]
fn newline_does_not_compound_indent_inside_an_outer_rust_block(cx: &mut TestAppContext) {
    let source = "pub(crate) fn config_dir() -> &'static Path {\n    static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();\n    CONFIG_DIR.get_or_init(|| home_dir().join(\".zcv\")).as_path()\n}";
    let caret = source.find(".as_path()").unwrap() + ".as_path()".len();
    let raw_buffer = cx.new(|_| {
        Buffer::scratch(source.to_owned(), BufferConfig::default())
            .expect("Rust 测试 Buffer 应能创建")
    });
    let language_buffer = cx.new({
        let raw_buffer = raw_buffer.clone();
        move |cx| LanguageBuffer::new(raw_buffer, Some(PathBuf::from("main.rs")), cx)
    });
    let editor = cx.new({
        let language_buffer = language_buffer.clone();
        move |cx| {
            let mut editor = Editor::for_language_buffer(language_buffer, cx);
            editor.set_selections(SelectionSet::caret(ByteOffset::new(caret)));
            editor
        }
    });
    cx.run_until_parked();

    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));

    assert_eq!(
        buffer_text(&language_buffer, cx),
        "pub(crate) fn config_dir() -> &'static Path {\n    static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();\n    CONFIG_DIR.get_or_init(|| home_dir().join(\".zcv\")).as_path()\n    \n    \n}"
    );
}

#[gpui::test]
fn newline_uses_the_nearest_code_line_as_its_indent_basis(cx: &mut TestAppContext) {
    let source = "fn main() {\n    build()\n}";
    let caret = source.find("build(").unwrap() + "build(".len();
    let raw_buffer = cx.new(|_| {
        Buffer::scratch(source.to_owned(), BufferConfig::default())
            .expect("Rust 测试 Buffer 应能创建")
    });
    let language_buffer = cx.new({
        let raw_buffer = raw_buffer.clone();
        move |cx| LanguageBuffer::new(raw_buffer, Some(PathBuf::from("main.rs")), cx)
    });
    let editor = cx.new({
        let language_buffer = language_buffer.clone();
        move |cx| {
            let mut editor = Editor::for_language_buffer(language_buffer, cx);
            editor.set_selections(SelectionSet::caret(ByteOffset::new(caret)));
            editor
        }
    });
    cx.run_until_parked();

    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));

    assert_eq!(
        buffer_text(&language_buffer, cx),
        "fn main() {\n    build(\n        \n        \n    )\n}"
    );
}
