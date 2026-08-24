use gpui::{EntityInputHandler, TestAppContext, px, size};
use zcv_text::ByteOffset;

use super::common::{buffer_text, test_buffer};
use super::*;
use crate::SelectionSet;

#[gpui::test]
fn marked_text_updates_buffer_and_unmark_finishes_composition(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "ab");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| {
            let mut editor = Editor::for_language_buffer(buffer, cx);
            editor.set_selections(SelectionSet::caret(ByteOffset::new(1)));
            editor
        }
    });

    cx.update(|window, app| {
        editor.update(app, |editor, cx| {
            editor.replace_and_mark_text_in_range(None, "中文😀", Some(2..2), window, cx);
        });
    });
    cx.refresh().expect("测试窗口应可刷新");
    cx.run_until_parked();

    assert_eq!(buffer_text(&buffer, cx), "a中文😀b");
    cx.update(|window, app| {
        editor.update(app, |editor, cx| {
            let marked = editor
                .marked_text_range(window, cx)
                .expect("应存在 marked range");
            let selected = editor
                .selected_text_range(false, window, cx)
                .expect("应存在 composition 相对选区");
            assert_eq!(marked, 1..5);
            assert_eq!(selected.range, 3..3);
            assert!(
                editor
                    .bounds_for_range(marked.end..marked.end, Bounds::default(), window, cx)
                    .is_some()
            );
            editor.unmark_text(window, cx);
        });
    });

    assert_eq!(buffer_text(&buffer, cx), "a中文😀b");
    cx.read_entity(&editor, |editor, _| {
        assert!(editor.composition.is_none());
        assert_eq!(editor.selections().primary().head(), ByteOffset::new(7));
    });
}
#[gpui::test]
fn ime_candidate_updates_merge_into_one_undo_step(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "ab");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| {
            let mut editor = Editor::for_language_buffer(buffer, cx);
            editor.set_selections(SelectionSet::caret(ByteOffset::new(1)));
            editor
        }
    });

    cx.update(|window, app| {
        editor.update(app, |editor, cx| {
            editor.replace_and_mark_text_in_range(None, "z", None, window, cx);
            editor.replace_and_mark_text_in_range(None, "zh", None, window, cx);
            editor.replace_and_mark_text_in_range(None, "中", None, window, cx);
            editor.unmark_text(window, cx);
        });
    });
    assert_eq!(buffer_text(&buffer, cx), "a中b");

    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), "ab");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), SelectionSet::caret(ByteOffset::new(1)));
    });

    cx.update_entity(&editor, |editor, cx| editor.redo(cx));
    assert_eq!(buffer_text(&buffer, cx), "a中b");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), SelectionSet::caret(ByteOffset::new(4)));
    });
}
#[gpui::test]
fn ime_updates_every_cursor_and_tracks_the_primary_marked_range(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "ab cd");
    let initial_selections = SelectionSet::new_with_primary(
        vec![
            Selection::caret(ByteOffset::new(1)),
            Selection::caret(ByteOffset::new(4)),
        ],
        1,
    );
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        let initial_selections = initial_selections.clone();
        move |_, cx| {
            let mut editor = Editor::for_language_buffer(buffer, cx);
            editor.set_selections(initial_selections);
            editor
        }
    });

    cx.update(|window, app| {
        editor.update(app, |editor, cx| {
            editor.replace_and_mark_text_in_range(None, "中", None, window, cx);
            assert_eq!(editor.composition.as_ref().unwrap().ranges.len(), 2);
            assert_eq!(editor.marked_text_range(window, cx), Some(5..6));
            editor.replace_and_mark_text_in_range(None, "文", None, window, cx);
            editor.unmark_text(window, cx);
        });
    });

    assert_eq!(buffer_text(&buffer, cx), "a文b c文d");
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), "ab cd");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), initial_selections);
    });
}
#[gpui::test]
fn ime_candidate_remains_in_the_syntax_highlight_pipeline(cx: &mut TestAppContext) {
    let source = "fn main() { let value = \"\"; }";
    let insertion = source.find("\"\"").unwrap() + 1;
    let raw_buffer = Buffer::scratch(source.to_owned(), BufferConfig::default())
        .expect("Rust 测试 Buffer 应能创建");
    let raw_buffer = cx.new(|_| raw_buffer);
    let language_buffer = cx.new({
        let raw_buffer = raw_buffer.clone();
        move |cx| LanguageBuffer::new(raw_buffer, Some(PathBuf::from("main.rs")), cx)
    });
    let (editor, cx) = cx.add_window_view({
        let language_buffer = language_buffer.clone();
        move |_, cx| {
            let mut editor = Editor::for_language_buffer(language_buffer, cx);
            editor.set_selections(SelectionSet::caret(ByteOffset::new(insertion)));
            editor
        }
    });
    cx.run_until_parked();

    cx.update(|window, app| {
        editor.update(app, |editor, cx| {
            editor.replace_and_mark_text_in_range(None, "中文", None, window, cx);
        });
    });
    cx.run_until_parked();

    cx.read_entity(&editor, |editor, cx| {
        let snapshot = editor.singleton_buffer(cx).read(cx).snapshot();
        let composition = editor.composition.as_ref().unwrap();
        let marked = composition.ranges[composition.primary_index];
        let syntax_snapshot = editor.display_map.syntax_snapshot();
        let names = syntax_snapshot.capture_names();
        let highlights = syntax_snapshot.highlights(0..snapshot.len_bytes().get(), &snapshot);
        assert!(highlights.iter().any(|highlight| {
            names[highlight.capture as usize].as_ref() == "string"
                && highlight.range.start <= marked.start().get()
                && highlight.range.end >= marked.end().get()
        }));
    });
}
#[gpui::test]
fn ime_relative_utf16_range_replaces_the_marked_subrange(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.update(|window, app| {
        editor.update(app, |editor, cx| {
            editor.replace_and_mark_text_in_range(None, "a😀b", None, window, cx);
            editor.replace_and_mark_text_in_range(Some(1..3), "中", None, window, cx);
        });
    });

    assert_eq!(buffer_text(&buffer, cx), "a中b");
    cx.update(|window, app| {
        editor.update(app, |editor, cx| {
            assert_eq!(editor.marked_text_range(window, cx), Some(1..2));
            editor.unmark_text(window, cx);
        });
    });
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), "");
}
#[gpui::test]
fn marked_text_can_cancel_and_committed_range_uses_utf16_offsets(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "a😀b");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.update(|window, app| {
        editor.update(app, |editor, cx| {
            editor.replace_and_mark_text_in_range(None, "候选", None, window, cx);
            editor.replace_text_in_range(None, "", window, cx);
            assert!(editor.composition.is_none());
            editor.replace_text_in_range(Some(1..3), "你", window, cx);
        });
    });

    assert_eq!(buffer_text(&buffer, cx), "a你b");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections().primary().head(), ByteOffset::new(4));
    });
}
#[gpui::test]
fn ime_candidate_bounds_survive_composition_and_scroll_layout_invalidation(
    cx: &mut TestAppContext,
) {
    let text = (0..40)
        .map(|row| format!("line {row}\n"))
        .collect::<String>();
    let buffer = test_buffer(cx, text);
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    let element_bounds = Bounds::new(point(px(100.), px(200.)), size(px(500.), px(300.)));
    let caret_bounds = Bounds::new(point(px(124.), px(260.)), size(px(2.), px(20.)));

    cx.update(|window, app| {
        editor.update(app, |editor, cx| {
            editor.set_ime_caret_geometry(element_bounds, Some(caret_bounds));
            editor.replace_and_mark_text_in_range(None, "中文", Some(2..2), window, cx);
            assert!(editor.input_layout.is_none());
            assert_eq!(
                editor.bounds_for_range(2..2, element_bounds, window, cx),
                Some(caret_bounds)
            );

            editor.prepare_scroll_viewport(size(px(100.), px(100.)), px(200.), px(20.));
            assert!(editor.scroll_by(point(px(0.), px(-60.)), cx));
            assert_eq!(
                editor.bounds_for_range(2..2, element_bounds, window, cx),
                Some(caret_bounds)
            );
        });
    });
}
