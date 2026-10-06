use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{ExcerptRange, MultiBuffer, MultiBufferOffset, MultiBufferRange};
use zcv_project::SearchQuery;
use zcv_text::{Buffer, BufferConfig};
use zcv_workspace::SearchableItem;

use super::common::{buffer_text, focus_editor};
use super::*;
use crate::selection::{Selection, SelectionSet};

fn editor_with_path<'a>(
    cx: &'a mut TestAppContext,
    path: &'static str,
    text: &str,
) -> (
    Entity<LanguageBuffer>,
    Entity<Editor>,
    &'a mut VisualTestContext,
) {
    let buffer = Buffer::from_text(text.to_owned(), BufferConfig::default()).unwrap();
    let source = cx.new(move |cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from(path)),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let (editor, cx) = cx.add_window_view({
        let source = source.clone();
        move |_, cx| Editor::for_language_buffer(source, cx)
    });
    (source, editor, cx)
}

fn caret(editor: &Entity<Editor>, cx: &mut VisualTestContext, at: usize) {
    editor.update(cx, |editor, cx| {
        editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(at)), cx);
    });
}

fn head(editor: &Entity<Editor>, cx: &VisualTestContext) -> usize {
    cx.read_entity(editor, |editor, cx| {
        editor.selections(cx).primary().head().get()
    })
}

fn double_click(editor: &Entity<Editor>, cx: &mut VisualTestContext, at: usize) {
    editor.update(cx, |editor, cx| {
        let display = editor.display_snapshot(cx);
        let point = display
            .offset_to_display_point(MultiBufferOffset::new(at))
            .unwrap();
        editor.begin_selection(point, 2, false, cx);
    });
}

#[gpui::test]
fn word_actions_use_code_string_and_comment_scopes(cx: &mut TestAppContext) {
    let source = "const foo$bar = \"foo.bar\"; // foo-bar\n";
    let (_, editor, cx) = editor_with_path(cx, "main.js", source);
    cx.run_until_parked();
    focus_editor(&editor, cx);
    for word in ["foo$bar", "foo.bar", "foo-bar"] {
        let start = source.find(word).unwrap();
        caret(&editor, cx, start);
        cx.dispatch_action(MoveToNextWord);
        assert_eq!(head(&editor, cx), start + word.len(), "{word}");
        cx.dispatch_action(MoveToPreviousWord);
        assert_eq!(head(&editor, cx), start, "{word}");
        cx.dispatch_action(SelectToNextWord);
        cx.read_entity(&editor, |editor, cx| {
            assert_eq!(
                editor.selections(cx).primary().range(),
                MultiBufferRange::new(
                    MultiBufferOffset::new(start),
                    MultiBufferOffset::new(start + word.len()),
                )
                .unwrap(),
                "{word}"
            );
        });
        double_click(&editor, cx, start + 4);
        cx.read_entity(&editor, |editor, cx| {
            assert_eq!(
                editor.selections(cx).primary().range(),
                MultiBufferRange::new(
                    MultiBufferOffset::new(start),
                    MultiBufferOffset::new(start + word.len()),
                )
                .unwrap(),
                "{word}"
            );
        });
        editor.update(cx, |editor, _| editor.end_selection());
    }
}

#[gpui::test]
fn word_delete_with_selection_restores_text_and_caret_on_undo_redo(cx: &mut TestAppContext) {
    let source = "const foo$bar = 1;";
    let (buffer, editor, cx) = editor_with_path(cx, "main.js", source);
    cx.run_until_parked();
    focus_editor(&editor, cx);
    let start = source.find("foo$bar").unwrap();
    editor.update(cx, |editor, cx| {
        editor.set_selections(
            SelectionSet::new(vec![Selection::new(
                MultiBufferOffset::new(start),
                MultiBufferOffset::new(start + "foo$bar".len()),
            )]),
            cx,
        );
    });
    cx.dispatch_action(DeleteToNextWordEnd);
    assert_eq!(buffer_text(&buffer, cx), "const  = 1;");
    assert_eq!(head(&editor, cx), start);
    editor.update(cx, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), source);
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().range(),
            MultiBufferRange::new(
                MultiBufferOffset::new(start),
                MultiBufferOffset::new(start + "foo$bar".len()),
            )
            .unwrap()
        );
    });
    editor.update(cx, |editor, cx| editor.redo(cx));
    assert_eq!(buffer_text(&buffer, cx), "const  = 1;");
    assert_eq!(head(&editor, cx), start);
}

#[gpui::test]
fn forward_and_backward_word_delete_use_current_scope(cx: &mut TestAppContext) {
    let source = "const x = \"foo.bar\"; // foo-bar";
    let (buffer, editor, cx) = editor_with_path(cx, "main.js", source);
    cx.run_until_parked();
    focus_editor(&editor, cx);
    let string_start = source.find("foo.bar").unwrap();
    caret(&editor, cx, string_start);
    cx.dispatch_action(DeleteToNextWordEnd);
    assert_eq!(buffer_text(&buffer, cx), "const x = \"\"; // foo-bar");
    assert_eq!(head(&editor, cx), string_start);
    editor.update(cx, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), source);
    assert_eq!(head(&editor, cx), string_start);

    let comment_end = source.len();
    caret(&editor, cx, comment_end);
    cx.dispatch_action(DeleteToPreviousWordStart);
    assert_eq!(buffer_text(&buffer, cx), "const x = \"foo.bar\"; // ");
    assert_eq!(head(&editor, cx), source.find("foo-bar").unwrap());
    editor.update(cx, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), source);
    assert_eq!(head(&editor, cx), comment_end);
    cx.read_entity(&editor, |editor, cx| {
        let display = editor.display_snapshot(cx);
        let snapshot = display.buffer_snapshot();
        let at = snapshot
            .byte_to_char(MultiBufferOffset::new(comment_end))
            .unwrap();
        let (start, end) = snapshot.surrounding_word(at).unwrap();
        assert_eq!(
            snapshot.char_to_byte(start).unwrap().get(),
            source.find("foo-bar").unwrap()
        );
        assert_eq!(snapshot.char_to_byte(end).unwrap().get(), comment_end);
    });
    double_click(&editor, cx, comment_end);
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().range(),
            MultiBufferRange::new(
                MultiBufferOffset::new(source.find("foo-bar").unwrap()),
                MultiBufferOffset::new(comment_end),
            )
            .unwrap()
        );
    });
}

#[gpui::test]
fn unicode_word_scope_respects_graphemes_and_document_edges(cx: &mut TestAppContext) {
    let source = "const x = \"e\u{301}.中\";\n";
    let (_, editor, cx) = editor_with_path(cx, "main.js", source);
    cx.run_until_parked();
    focus_editor(&editor, cx);
    let start = source.find("e\u{301}.中").unwrap();
    let end = start + "e\u{301}.中".len();
    caret(&editor, cx, start);
    cx.dispatch_action(MoveToNextWord);
    assert_eq!(head(&editor, cx), end);
    double_click(&editor, cx, source.find('中').unwrap());
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().range(),
            MultiBufferRange::new(MultiBufferOffset::new(start), MultiBufferOffset::new(end))
                .unwrap()
        );
    });
    editor.update(cx, |editor, _| editor.end_selection());
    caret(&editor, cx, 0);
    cx.dispatch_action(MoveToPreviousWord);
    assert_eq!(head(&editor, cx), 0);
    caret(&editor, cx, source.len());
    cx.dispatch_action(MoveToNextWord);
    assert_eq!(head(&editor, cx), source.len());
}

#[gpui::test]
fn markdown_injection_uses_inner_word_scope_for_movement_and_selection(cx: &mut TestAppContext) {
    let source = "```javascript\nconst x = \"foo.bar\";\n```\n";
    let (_, editor, cx) = editor_with_path(cx, "README.md", source);
    cx.run_until_parked();
    focus_editor(&editor, cx);
    let start = source.find("foo.bar").unwrap();
    caret(&editor, cx, start);
    cx.dispatch_action(MoveToNextWord);
    assert_eq!(head(&editor, cx), start + "foo.bar".len());
    double_click(&editor, cx, start + 4);
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().range(),
            MultiBufferRange::new(
                MultiBufferOffset::new(start),
                MultiBufferOffset::new(start + "foo.bar".len()),
            )
            .unwrap()
        );
    });
}

#[gpui::test]
fn nested_markdown_html_script_word_actions_follow_inner_string_scope(cx: &mut TestAppContext) {
    let source = "<script>const x = \"foo.bar\";</script>\n";
    let (buffer, editor, cx) = editor_with_path(cx, "README.md", source);
    cx.run_until_parked();
    focus_editor(&editor, cx);
    let start = source.find("foo.bar").unwrap();
    let end = start + "foo.bar".len();
    caret(&editor, cx, start);
    cx.dispatch_action(MoveToNextWord);
    assert_eq!(head(&editor, cx), end);
    double_click(&editor, cx, start + 4);
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().range(),
            MultiBufferRange::new(MultiBufferOffset::new(start), MultiBufferOffset::new(end))
                .unwrap()
        );
    });
    cx.dispatch_action(DeleteToNextWordEnd);
    assert_eq!(
        buffer_text(&buffer, cx),
        "<script>const x = \"\";</script>\n"
    );
    assert_eq!(head(&editor, cx), start);
    editor.update(cx, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), source);
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().range(),
            MultiBufferRange::new(MultiBufferOffset::new(start), MultiBufferOffset::new(end))
                .unwrap()
        );
    });
}

#[gpui::test]
fn word_movement_handles_line_start_and_line_end(cx: &mut TestAppContext) {
    let source = "foo$bar\nbaz";
    let (_, editor, cx) = editor_with_path(cx, "main.js", source);
    cx.run_until_parked();
    focus_editor(&editor, cx);
    caret(&editor, cx, 8);
    cx.dispatch_action(MoveToNextWord);
    assert_eq!(head(&editor, cx), source.len());
    cx.dispatch_action(MoveToPreviousWord);
    assert_eq!(head(&editor, cx), 8);
}

#[gpui::test]
fn language_change_recomputes_word_policy_without_a_source_copy(cx: &mut TestAppContext) {
    let (buffer, editor, cx) = editor_with_path(cx, "main.js", "foo$bar");
    cx.run_until_parked();
    focus_editor(&editor, cx);
    caret(&editor, cx, 0);
    cx.dispatch_action(MoveToNextWord);
    assert_eq!(head(&editor, cx), 7);

    buffer.update(cx, |buffer, cx| buffer.set_file_path("main.rs".into(), cx));
    cx.run_until_parked();
    caret(&editor, cx, 0);
    cx.dispatch_action(MoveToNextWord);
    assert_eq!(head(&editor, cx), 3);
}

#[gpui::test]
fn read_only_word_delete_keeps_text_and_selection(cx: &mut TestAppContext) {
    let source = "const x = \"foo.bar\";";
    let buffer = Buffer::from_text(source.to_owned(), BufferConfig::default()).unwrap();
    let buffer = cx.new(move |cx| {
        LanguageBuffer::new(
            buffer,
            Some("main.js".into()),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let combined = cx.new(MultiBuffer::empty_read_only);
    combined.update(cx, |combined, cx| {
        combined.set_excerpts_for_path(
            vec![ExcerptRange::new(
                buffer.clone(),
                MultiBufferRange::new(
                    MultiBufferOffset::ZERO,
                    MultiBufferOffset::new(source.len()),
                )
                .unwrap()
                .into(),
                Vec::new(),
            )],
            cx,
        );
    });
    let (editor, cx) = cx.add_window_view(move |_, cx| Editor::for_multi_buffer(combined, cx));
    cx.run_until_parked();
    focus_editor(&editor, cx);
    let start = source.find("foo.bar").unwrap();
    caret(&editor, cx, start);
    cx.dispatch_action(DeleteToNextWordEnd);
    assert_eq!(buffer_text(&buffer, cx), source);
    assert_eq!(head(&editor, cx), start);
}

#[gpui::test]
fn global_whole_word_search_uses_host_language_without_a_cursor(cx: &mut TestAppContext) {
    let source = "const x = \"foo.bar\";";
    let (_, editor, cx) = editor_with_path(cx, "main.js", source);
    cx.run_until_parked();
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.search(
                &SearchQuery {
                    query: "foo".into(),
                    whole_word: true,
                    ..Default::default()
                },
                window,
                cx,
            );
            assert_eq!(editor.search_count(cx).0, 1);
            assert_eq!(
                editor.search_highlights().unwrap().0,
                vec![
                    MultiBufferRange::new(
                        MultiBufferOffset::new(source.find("foo").unwrap()),
                        MultiBufferOffset::new(source.find("foo").unwrap() + 3),
                    )
                    .unwrap()
                ]
            );
        });
    });
}

#[gpui::test]
fn composite_excerpt_word_movement_stops_at_source_boundary(cx: &mut TestAppContext) {
    let first = Buffer::from_text("foo$bar".to_owned(), BufferConfig::default()).unwrap();
    let second = Buffer::from_text("baz".to_owned(), BufferConfig::default()).unwrap();
    let registry = Arc::new(LanguageRegistry::new());
    let first = cx.new({
        let registry = Arc::clone(&registry);
        move |cx| LanguageBuffer::new(first, Some("a.js".into()), registry, cx)
    });
    let second = cx.new(move |cx| LanguageBuffer::new(second, Some("b.rs".into()), registry, cx));
    let combined = cx.new(MultiBuffer::empty);
    combined.update(cx, |combined, cx| {
        for (source, len) in [(first.clone(), 7), (second.clone(), 3)] {
            combined.set_excerpts_for_path(
                vec![ExcerptRange::new(
                    source,
                    MultiBufferRange::new(MultiBufferOffset::ZERO, MultiBufferOffset::new(len))
                        .unwrap()
                        .into(),
                    Vec::new(),
                )],
                cx,
            );
        }
    });
    let (editor, cx) = cx.add_window_view(move |_, cx| Editor::for_multi_buffer(combined, cx));
    cx.run_until_parked();
    focus_editor(&editor, cx);
    let combined_text = cx.read_entity(&editor, |editor, cx| {
        String::from_utf8(editor.display_snapshot(cx).buffer_snapshot().text_bytes()).unwrap()
    });
    assert_eq!(combined_text, "foo$bar\nbaz");
    cx.read_entity(&editor, |editor, cx| {
        let display = editor.display_snapshot(cx);
        let snapshot = display.buffer_snapshot();
        assert_eq!(
            snapshot
                .input_scope_at(MultiBufferOffset::ZERO)
                .unwrap()
                .language_name(),
            "JavaScript"
        );
        assert_eq!(
            snapshot
                .char_to_byte(
                    snapshot
                        .movement_boundary(
                            zcv_text::CharOffset::ZERO,
                            zcv_text::MovementDirection::Next,
                            zcv_text::MovementUnit::Word
                        )
                        .unwrap()
                )
                .unwrap()
                .get(),
            7
        );
    });
    caret(&editor, cx, 0);
    cx.dispatch_action(MoveToNextWord);
    // 原始词终点是 7；结构分隔换行不可停靠，Editor 锚点落到下一源起点 8。
    assert_eq!(head(&editor, cx), 8);
    cx.dispatch_action(MoveToNextWord);
    assert_eq!(head(&editor, cx), 11);
    double_click(&editor, cx, 9);
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().range(),
            MultiBufferRange::new(MultiBufferOffset::new(8), MultiBufferOffset::new(11)).unwrap()
        );
    });
}
