use std::sync::Arc;

use gpui::{AppContext, TestAppContext, VisualTestContext};
use zcv_workspace::SearchableItem;

use super::*;

fn search_bar_with_text<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
    query: &str,
    replacement: &str,
    options: MatchOptions,
) -> (Entity<Editor>, Entity<SearchBar>, &'a mut VisualTestContext) {
    let registry = Arc::new(LanguageRegistry::new());
    let (editor, cx) = cx.add_window_view({
        let registry = Arc::clone(&registry);
        move |_, cx| Editor::single_line(registry, cx)
    });
    editor.update(cx, |editor, cx| editor.set_text(text, cx));
    let bar = cx.new(|cx| {
        SearchBar::new(
            SearchBarConfig {
                id_prefix: "test-search",
                key_context: "BufferSearchBar",
                supports_replace: true,
                query_placeholder: "搜索...",
                replace_placeholder: "替换为...",
                dismissible: true,
            },
            registry,
            cx,
        )
    });
    cx.update(|window, cx| {
        bar.update(cx, |bar, cx| {
            bar.restore(query, options, cx);
            bar.set_target(Some(Box::new(editor.downgrade())), window, cx);
            bar.replace_input
                .update(cx, |editor, cx| editor.set_text(replacement, cx));
            bar.deploy(None, window, cx);
        });
    });
    (editor, bar, cx)
}

#[gpui::test]
fn replace_next_visits_each_remaining_match_in_order(cx: &mut TestAppContext) {
    let (editor, bar, cx) =
        search_bar_with_text(cx, "abc abc abc", "abc", "X", MatchOptions::default());
    for (text, count) in [("X abc abc", 2), ("X X abc", 1), ("X X X", 0)] {
        cx.update(|window, cx| {
            bar.update(cx, |bar, cx| bar.replace_next(window, cx));
        });
        cx.read_entity(&editor, |editor, cx| {
            assert_eq!(editor.text(cx), text);
            assert_eq!(editor.search_count(cx), (count, (count > 0).then_some(0)));
            if count > 0 {
                assert_eq!(editor.query_suggestion(cx).as_deref(), Some("abc"));
            }
        });
    }
}

#[gpui::test]
fn replace_next_wraps_after_the_last_match(cx: &mut TestAppContext) {
    let (editor, bar, cx) =
        search_bar_with_text(cx, "abc abc abc", "abc", "X", MatchOptions::default());
    cx.update(|window, cx| {
        bar.update(cx, |bar, cx| {
            bar.move_active(Direction::Next, window, cx);
            bar.move_active(Direction::Next, window, cx);
            bar.replace_next(window, cx);
        });
    });
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "abc abc X");
        assert_eq!(editor.search_count(cx), (2, Some(0)));
        assert_eq!(editor.query_suggestion(cx).as_deref(), Some("abc"));
    });
    cx.update(|window, cx| {
        bar.update(cx, |bar, cx| bar.replace_next(window, cx));
    });
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "X abc X");
    });
}

#[gpui::test]
fn replace_next_skips_matches_created_inside_the_replacement(cx: &mut TestAppContext) {
    let (editor, bar, cx) =
        search_bar_with_text(cx, "abc abc abc", "abc", "abcabc", MatchOptions::default());
    cx.update(|window, cx| {
        bar.update(cx, |bar, cx| bar.replace_next(window, cx));
    });
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "abcabc abc abc");
        assert_eq!(editor.search_count(cx), (4, Some(2)));
    });
    cx.update(|window, cx| {
        bar.update(cx, |bar, cx| bar.replace_next(window, cx));
    });
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(editor.text(cx), "abcabc abcabc abc");
        assert_eq!(editor.search_count(cx), (5, Some(4)));
    });
}

#[gpui::test]
fn replace_next_deletes_adjacent_matches_in_order(cx: &mut TestAppContext) {
    let (editor, bar, cx) =
        search_bar_with_text(cx, "abcabcabc", "abc", "", MatchOptions::default());
    for (text, count) in [("abcabc", 2), ("abc", 1), ("", 0)] {
        cx.update(|window, cx| {
            bar.update(cx, |bar, cx| bar.replace_next(window, cx));
        });
        cx.read_entity(&editor, |editor, cx| {
            assert_eq!(editor.text(cx), text);
            assert_eq!(editor.search_count(cx), (count, (count > 0).then_some(0)));
        });
    }
}

#[gpui::test]
fn replace_next_uses_regex_capture_replacement_lengths(cx: &mut TestAppContext) {
    let (editor, bar, cx) = search_bar_with_text(
        cx,
        "a1 a22 a333",
        r"a(\d+)",
        "[$1]",
        MatchOptions {
            regex: true,
            ..MatchOptions::default()
        },
    );
    for (text, count) in [
        ("[1] a22 a333", 2),
        ("[1] [22] a333", 1),
        ("[1] [22] [333]", 0),
    ] {
        cx.update(|window, cx| {
            bar.update(cx, |bar, cx| bar.replace_next(window, cx));
        });
        cx.read_entity(&editor, |editor, cx| {
            assert_eq!(editor.text(cx), text);
            assert_eq!(editor.search_count(cx), (count, (count > 0).then_some(0)));
        });
    }
}
