//! 自动闭合配对行为测试：自动补全闭合符、跳过已存在闭合符、包裹选区、退格删除整对与撤销回放。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::path::PathBuf;

use gpui::{AppContext, BorrowAppContext, EntityInputHandler, TestAppContext, VisualTestContext};
use zcv_actions::Backspace;
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{ExcerptRange, MultiBuffer};
use zcv_text::{Buffer, BufferConfig};

use super::Editor;
use crate::selection::{Selection, SelectionSet};

/// 按路径识别语言的窗口化编辑器。
fn editor_with_path<'a>(
    cx: &'a mut TestAppContext,
    path: &'static str,
    text: &str,
    selections: SelectionSet,
) -> (
    gpui::Entity<LanguageBuffer>,
    gpui::Entity<Editor>,
    &'a mut VisualTestContext,
) {
    let buffer =
        Buffer::from_text(text.to_string(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let language_buffer = cx.new(move |cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from(path)),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let editor = cx.add_window_view({
        let language_buffer = language_buffer.clone();
        move |_, cx| {
            let mut editor = Editor::for_language_buffer(language_buffer, cx);
            editor.set_selections(selections, cx);
            editor
        }
    });
    (language_buffer, editor.0, editor.1)
}

fn editor_with_rust<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
    selections: SelectionSet,
) -> (
    gpui::Entity<LanguageBuffer>,
    gpui::Entity<Editor>,
    &'a mut VisualTestContext,
) {
    editor_with_path(cx, "test.rs", text, selections)
}

/// 不带语言的编辑器（无配对表，输入应原样插入）。
fn editor_without_language<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
    selections: SelectionSet,
) -> (
    gpui::Entity<LanguageBuffer>,
    gpui::Entity<Editor>,
    &'a mut VisualTestContext,
) {
    let buffer =
        Buffer::from_text(text.to_string(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let language_buffer = cx.new(move |cx| {
        LanguageBuffer::new(
            buffer,
            None,
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let editor = cx.add_window_view({
        let language_buffer = language_buffer.clone();
        move |_, cx| {
            let mut editor = Editor::for_language_buffer(language_buffer, cx);
            editor.set_selections(selections, cx);
            editor
        }
    });
    (language_buffer, editor.0, editor.1)
}

fn buffer_text(buffer: &gpui::Entity<LanguageBuffer>, cx: &VisualTestContext) -> String {
    cx.read_entity(buffer, |language_buffer, _| {
        let snapshot = language_buffer.text_snapshot();
        snapshot
            .slice_byte_range(MultiBufferOffset::ZERO.into(), snapshot.len_bytes())
            .expect("完整测试范围应可读取")
            .as_str()
            .to_string()
    })
}

fn primary_head(editor: &gpui::Entity<Editor>, cx: &VisualTestContext) -> MultiBufferOffset {
    cx.read_entity(editor, |editor, cx| editor.selections(cx).primary().head())
}

fn assert_newline_roundtrip(
    buffer: &gpui::Entity<LanguageBuffer>,
    editor: &gpui::Entity<Editor>,
    cx: &mut VisualTestContext,
    original: &str,
    expected: &str,
    before: usize,
    after: usize,
) {
    cx.update_entity(editor, |editor, cx| editor.insert_newline(cx));
    assert_eq!(buffer_text(buffer, cx), expected);
    assert_eq!(primary_head(editor, cx), MultiBufferOffset::new(after));
    assert!(cx.read_entity(editor, |editor, cx| {
        editor.selections(cx).primary().is_caret()
    }));
    cx.update_entity(editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(buffer, cx), original);
    assert_eq!(primary_head(editor, cx), MultiBufferOffset::new(before));
    cx.update_entity(editor, |editor, cx| editor.redo(cx));
    assert_eq!(buffer_text(buffer, cx), expected);
    assert_eq!(primary_head(editor, cx), MultiBufferOffset::new(after));
}

fn type_text(editor: &gpui::Entity<Editor>, cx: &mut VisualTestContext, text: &str) {
    cx.update_entity(editor, |editor, cx| {
        editor.replace_text(None, text, cx);
    });
}

fn backspace(editor: &gpui::Entity<Editor>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.handle_backspace(&Backspace, window, cx);
        });
    });
}

#[gpui::test]
fn each_composite_selection_uses_its_source_language_pairs(cx: &mut TestAppContext) {
    let plain = cx.new(|cx| {
        LanguageBuffer::new(
            Buffer::from_text("x ".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建"),
            None,
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let rust = cx.new(|cx| {
        LanguageBuffer::new(
            Buffer::from_text("y ".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建"),
            Some(PathBuf::from("test.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(
            vec![ExcerptRange::new(
                plain.clone(),
                MultiBufferRange::new(MultiBufferOffset::ZERO, MultiBufferOffset::new(2))
                    .unwrap()
                    .into(),
                Vec::new(),
            )],
            cx,
        );
        buffer.set_excerpts_for_path(
            vec![ExcerptRange::new(
                rust.clone(),
                MultiBufferRange::new(MultiBufferOffset::ZERO, MultiBufferOffset::new(2))
                    .unwrap()
                    .into(),
                Vec::new(),
            )],
            cx,
        );
    });
    let (editor, cx) = cx.add_window_view({
        let combined = combined.clone();
        move |_, cx| {
            let mut editor = Editor::for_multi_buffer(combined, cx);
            editor.set_selections(
                SelectionSet::new(vec![
                    Selection::caret(MultiBufferOffset::new(1)),
                    Selection::caret(MultiBufferOffset::new(4)),
                ]),
                cx,
            );
            editor
        }
    });
    cx.run_until_parked();

    type_text(&editor, cx, "(");

    assert_eq!(buffer_text(&plain, cx), "x( ");
    assert_eq!(buffer_text(&rust, cx), "y() ");
    cx.read_entity(&editor, |editor, cx| {
        let heads: Vec<_> = editor
            .selections(cx)
            .as_slice()
            .iter()
            .map(|selection| selection.head())
            .collect();
        assert_eq!(
            heads,
            vec![MultiBufferOffset::new(2), MultiBufferOffset::new(6)]
        );
    });
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&plain, cx), "x ");
    assert_eq!(buffer_text(&rust, cx), "y ");
    cx.read_entity(&editor, |editor, cx| {
        let heads: Vec<_> = editor
            .selections(cx)
            .as_slice()
            .iter()
            .map(|selection| selection.head())
            .collect();
        assert_eq!(
            heads,
            vec![MultiBufferOffset::new(1), MultiBufferOffset::new(4)]
        );
    });
}

#[gpui::test]
fn composite_rust_selections_use_each_sources_current_scope(cx: &mut TestAppContext) {
    let registry = std::sync::Arc::new(LanguageRegistry::new());
    let first = cx.new({
        let registry = registry.clone();
        move |cx| {
            LanguageBuffer::new(
                Buffer::from_text("x  ".to_owned(), BufferConfig::default()).unwrap(),
                Some(PathBuf::from("a.rs")),
                registry,
                cx,
            )
        }
    });
    let second = cx.new(move |cx| {
        LanguageBuffer::new(
            Buffer::from_text("\"ab\"".to_owned(), BufferConfig::default()).unwrap(),
            Some(PathBuf::from("b.rs")),
            registry,
            cx,
        )
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        for source in [&first, &second] {
            let len = source.read(cx).text_snapshot().len_bytes();
            buffer.set_excerpts_for_path(
                vec![ExcerptRange::new(
                    source.clone(),
                    MultiBufferRange::new(
                        MultiBufferOffset::ZERO,
                        MultiBufferOffset::new(len.get()),
                    )
                    .unwrap()
                    .into(),
                    Vec::new(),
                )],
                cx,
            );
        }
    });
    let (editor, cx) = cx.add_window_view({
        let combined = combined.clone();
        move |_, cx| {
            let mut editor = Editor::for_multi_buffer(combined, cx);
            editor.set_selections(
                SelectionSet::new(vec![
                    Selection::caret(MultiBufferOffset::new(2)),
                    Selection::caret(MultiBufferOffset::new(6)),
                ]),
                cx,
            );
            editor
        }
    });
    cx.run_until_parked();
    type_text(&editor, cx, "\"");
    assert_eq!(buffer_text(&first, cx), "x \"\" ");
    assert_eq!(buffer_text(&second, cx), "\"a\"b\"");
    cx.read_entity(&editor, |editor, cx| {
        let heads: Vec<_> = editor
            .selections(cx)
            .as_slice()
            .iter()
            .map(|selection| selection.head())
            .collect();
        assert_eq!(
            heads,
            vec![MultiBufferOffset::new(3), MultiBufferOffset::new(9)]
        );
    });
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&first, cx), "x  ");
    assert_eq!(buffer_text(&second, cx), "\"ab\"");
    cx.read_entity(&editor, |editor, cx| {
        let heads: Vec<_> = editor
            .selections(cx)
            .as_slice()
            .iter()
            .map(|selection| selection.head())
            .collect();
        assert_eq!(
            heads,
            vec![MultiBufferOffset::new(2), MultiBufferOffset::new(6)]
        );
    });
}

#[gpui::test]
fn rust_string_disables_quote_pair_but_comment_keeps_brackets(cx: &mut TestAppContext) {
    let original = "let s = \"ab\"; // ";
    let quote_position = original.find("ab").unwrap() + 1;
    let (buffer, editor, cx) = editor_with_rust(
        cx,
        original,
        SelectionSet::caret(MultiBufferOffset::new(quote_position)),
    );
    cx.run_until_parked();
    type_text(&editor, cx, "\"");
    assert_eq!(buffer_text(&buffer, cx), "let s = \"a\"b\"; // ");
    assert_eq!(
        primary_head(&editor, cx),
        MultiBufferOffset::new(quote_position + 1)
    );
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), original);
    assert_eq!(
        primary_head(&editor, cx),
        MultiBufferOffset::new(quote_position)
    );

    let comment_position = original.find("// ").unwrap() + 3;
    cx.update_entity(&editor, |editor, cx| {
        editor.set_selections(
            SelectionSet::caret(MultiBufferOffset::new(comment_position)),
            cx,
        );
    });
    type_text(&editor, cx, "(");
    assert_eq!(buffer_text(&buffer, cx), "let s = \"ab\"; // ()");
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), original);
}

#[gpui::test]
fn rust_code_quote_pair_restores_text_and_caret_on_undo(cx: &mut TestAppContext) {
    let source = "let x = ;";
    let at = source.find(';').unwrap();
    let (buffer, editor, cx) =
        editor_with_rust(cx, source, SelectionSet::caret(MultiBufferOffset::new(at)));
    cx.run_until_parked();
    type_text(&editor, cx, "\"");
    assert_eq!(buffer_text(&buffer, cx), "let x = \"\";");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(at + 1));
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), source);
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(at));
}

#[gpui::test]
fn rust_multicharacter_start_autocloses_on_last_character(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_with_rust(cx, "", SelectionSet::caret(MultiBufferOffset::ZERO));
    type_text(&editor, cx, "r");
    type_text(&editor, cx, "#");
    type_text(&editor, cx, "\"");
    assert_eq!(buffer_text(&buffer, cx), "r#\"\"#");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(3));
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), "r#");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(2));
}

#[gpui::test]
fn complete_multicharacter_pair_input_and_manual_close_share_region(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_with_rust(cx, "", SelectionSet::caret(MultiBufferOffset::ZERO));
    type_text(&editor, cx, "r#\"");
    assert_eq!(buffer_text(&buffer, cx), "r#\"\"#");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(3));
    type_text(&editor, cx, "\"#");
    assert_eq!(buffer_text(&buffer, cx), "r#\"\"#");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(5));
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), "");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::ZERO);
}

#[gpui::test]
fn rapid_input_then_parse_install_uses_current_string_scope(cx: &mut TestAppContext) {
    let source = "let x = ;";
    let at = source.find(';').unwrap();
    let (buffer, editor, cx) =
        editor_with_rust(cx, source, SelectionSet::caret(MultiBufferOffset::new(at)));
    type_text(&editor, cx, "\"");
    type_text(&editor, cx, "a");
    assert_eq!(buffer_text(&buffer, cx), "let x = \"a\";");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(at + 2));

    cx.run_until_parked();
    cx.update_entity(&editor, |editor, cx| {
        editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(at + 1)), cx);
    });
    type_text(&editor, cx, "\"");
    assert_eq!(buffer_text(&buffer, cx), "let x = \"\"a\";");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(at + 2));
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), "let x = \"a\";");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(at + 1));
}

#[gpui::test]
fn markdown_fence_uses_injected_rust_pair_policy(cx: &mut TestAppContext) {
    let source = "```rust\nlet x = ;\n```\n";
    let at = source.find(" = ").unwrap() + 3;
    let (buffer, editor, cx) = editor_with_path(
        cx,
        "README.md",
        source,
        SelectionSet::caret(MultiBufferOffset::new(at)),
    );
    cx.run_until_parked();
    type_text(&editor, cx, "\"");
    assert_eq!(buffer_text(&buffer, cx), "```rust\nlet x = \"\";\n```\n");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(at + 1));
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), source);
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(at));
}

#[gpui::test]
fn markdown_empty_list_item_ends_the_list(cx: &mut TestAppContext) {
    let source = "- ";
    let (buffer, editor, cx) = editor_with_path(
        cx,
        "README.md",
        source,
        SelectionSet::caret(MultiBufferOffset::new(2)),
    );
    cx.run_until_parked();
    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    assert_eq!(buffer_text(&buffer, cx), "");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::ZERO);
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), source);
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(2));
    cx.update_entity(&editor, |editor, cx| editor.redo(cx));
    assert_eq!(buffer_text(&buffer, cx), "");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::ZERO);
}

#[gpui::test]
fn newline_continues_rust_comments_and_restores_with_undo_redo(cx: &mut TestAppContext) {
    let source = "// item";
    let (buffer, editor, cx) = editor_with_rust(
        cx,
        source,
        SelectionSet::caret(MultiBufferOffset::new(source.len())),
    );
    cx.run_until_parked();

    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    assert_eq!(buffer_text(&buffer, cx), "// item\n// ");
    assert_eq!(
        primary_head(&editor, cx),
        MultiBufferOffset::new("// item\n// ".len())
    );

    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), source);
    assert_eq!(
        primary_head(&editor, cx),
        MultiBufferOffset::new(source.len())
    );
    cx.update_entity(&editor, |editor, cx| editor.redo(cx));
    assert_eq!(buffer_text(&buffer, cx), "// item\n// ");
    assert_eq!(
        primary_head(&editor, cx),
        MultiBufferOffset::new("// item\n// ".len())
    );
}

#[gpui::test]
fn newline_continues_doc_and_block_comments(cx: &mut TestAppContext) {
    for (source, expected) in [
        ("/// item", "/// item\n/// "),
        ("/* item */", "/* item\n * \n */"),
    ] {
        let (buffer, editor, cx) = editor_with_rust(
            cx,
            source,
            SelectionSet::caret(MultiBufferOffset::new(source.find("item").unwrap() + 4)),
        );
        cx.run_until_parked();
        cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
        assert_eq!(buffer_text(&buffer, cx), expected);
        assert_eq!(
            primary_head(&editor, cx),
            MultiBufferOffset::new(
                source.find("item").unwrap() + 4 + if source.starts_with("/*") { 4 } else { 5 }
            )
        );
        cx.update_entity(&editor, |editor, cx| editor.undo(cx));
        assert_eq!(buffer_text(&buffer, cx), source);
        assert_eq!(
            primary_head(&editor, cx),
            MultiBufferOffset::new(source.find("item").unwrap() + 4)
        );
        cx.update_entity(&editor, |editor, cx| editor.redo(cx));
        assert_eq!(buffer_text(&buffer, cx), expected);
        assert_eq!(
            primary_head(&editor, cx),
            MultiBufferOffset::new(
                source.find("item").unwrap() + 4 + if source.starts_with("/*") { 4 } else { 5 }
            )
        );
    }
}

#[gpui::test]
fn newline_respects_comment_start_and_end_boundaries(cx: &mut TestAppContext) {
    for (source, cursor, expected, after) in [
        ("// item", 1, "/\n/ item", 2),
        ("/* */", 2, "/*\n * \n */", 6),
        ("/* */", 5, "/* */\n", 6),
    ] {
        let (buffer, editor, cx) = editor_with_rust(
            cx,
            source,
            SelectionSet::caret(MultiBufferOffset::new(cursor)),
        );
        cx.run_until_parked();
        assert_newline_roundtrip(&buffer, &editor, cx, source, expected, cursor, after);
    }
}

#[gpui::test]
fn css_and_html_without_documentation_policy_do_not_continue_block_comments(
    cx: &mut TestAppContext,
) {
    let source = "/* item */";
    let cursor = source.find("item").unwrap() + 4;
    let (buffer, editor, cx) = editor_with_path(
        cx,
        "theme.css",
        source,
        SelectionSet::caret(MultiBufferOffset::new(cursor)),
    );
    cx.run_until_parked();
    assert_newline_roundtrip(
        &buffer,
        &editor,
        cx,
        source,
        "/* item\n */",
        cursor,
        cursor + 1,
    );

    let source = "<!-- item -->";
    let cursor = source.find("item").unwrap() + 4;
    let (buffer, editor, cx) = editor_with_path(
        cx,
        "index.html",
        source,
        SelectionSet::caret(MultiBufferOffset::new(cursor)),
    );
    cx.run_until_parked();
    assert_newline_roundtrip(
        &buffer,
        &editor,
        cx,
        source,
        "<!-- item\n -->",
        cursor,
        cursor + 1,
    );
}

#[gpui::test]
fn markdown_lists_continue_ordered_unordered_and_tasks(cx: &mut TestAppContext) {
    for (source, expected) in [
        ("- item", "- item\n- "),
        ("1. item", "1. item\n2. "),
        ("- [x] item", "- [x] item\n- [ ] "),
    ] {
        let (buffer, editor, cx) = editor_with_path(
            cx,
            "README.md",
            source,
            SelectionSet::caret(MultiBufferOffset::new(source.len())),
        );
        cx.run_until_parked();
        cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
        assert_eq!(buffer_text(&buffer, cx), expected, "{source}");
        assert_eq!(
            primary_head(&editor, cx),
            MultiBufferOffset::new(expected.len())
        );
        cx.update_entity(&editor, |editor, cx| editor.undo(cx));
        assert_eq!(buffer_text(&buffer, cx), source);
        assert_eq!(
            primary_head(&editor, cx),
            MultiBufferOffset::new(source.len())
        );
        cx.update_entity(&editor, |editor, cx| editor.redo(cx));
        assert_eq!(buffer_text(&buffer, cx), expected);
        assert_eq!(
            primary_head(&editor, cx),
            MultiBufferOffset::new(expected.len())
        );
    }
}

#[gpui::test]
fn markdown_lists_continue_when_splitting_item_content(cx: &mut TestAppContext) {
    for (source, cursor, expected, after) in [
        ("- item", 4, "- it\n- em", 7),
        ("1. item", 5, "1. it\n2. em", 9),
        ("- [x] item", 8, "- [x] it\n- [ ] em", 15),
        ("- 你好", 5, "- 你\n- 好", 8),
        ("  - item", 6, "  - it\n  - em", 11),
    ] {
        let (buffer, editor, cx) = editor_with_path(
            cx,
            "README.md",
            source,
            SelectionSet::caret(MultiBufferOffset::new(cursor)),
        );
        cx.run_until_parked();
        assert_newline_roundtrip(&buffer, &editor, cx, source, expected, cursor, after);
    }
}

#[gpui::test]
fn markdown_fenced_code_does_not_continue_list_like_text(cx: &mut TestAppContext) {
    let source = "```rust\n- item\n```";
    let cursor = source.find("item").unwrap() + 2;
    let expected = "```rust\n- it\nem\n```";
    let after = "```rust\n- it\n".len();
    let (buffer, editor, cx) = editor_with_path(
        cx,
        "README.md",
        source,
        SelectionSet::caret(MultiBufferOffset::new(cursor)),
    );
    cx.run_until_parked();
    assert_newline_roundtrip(&buffer, &editor, cx, source, expected, cursor, after);
}

#[gpui::test]
fn markdown_empty_task_and_ordered_items_clear_or_unindent(cx: &mut TestAppContext) {
    for (source, expected) in [
        ("- [ ] ", ""),
        ("3. ", ""),
        ("  - [x] ", "- [ ] "),
        ("  3. ", "1. "),
    ] {
        let (buffer, editor, cx) = editor_with_path(
            cx,
            "README.md",
            source,
            SelectionSet::caret(MultiBufferOffset::new(source.len())),
        );
        cx.run_until_parked();
        assert_newline_roundtrip(
            &buffer,
            &editor,
            cx,
            source,
            expected,
            source.len(),
            expected.len(),
        );
    }
}

#[gpui::test]
fn newline_between_brackets_replaces_selection_with_one_transaction(cx: &mut TestAppContext) {
    let source = "{ x }";
    let expected = "{ \n    \n }";
    let (buffer, editor, cx) = editor_with_rust(
        cx,
        source,
        SelectionSet::new(vec![Selection::new(
            MultiBufferOffset::new(2),
            MultiBufferOffset::new(3),
        )]),
    );
    cx.run_until_parked();
    assert_newline_roundtrip(&buffer, &editor, cx, source, expected, 3, 7);
}

#[gpui::test]
fn newline_applies_two_comment_scopes_in_one_undo_step(cx: &mut TestAppContext) {
    let source = "// a\n// b";
    let expected = "// a\n// \n// b\n// ";
    let (buffer, editor, cx) = editor_with_rust(
        cx,
        source,
        SelectionSet::new(vec![
            Selection::caret(MultiBufferOffset::new(4)),
            Selection::caret(MultiBufferOffset::new(9)),
        ]),
    );
    cx.run_until_parked();
    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    assert_eq!(buffer_text(&buffer, cx), expected);
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| {
            editor
                .selections(cx)
                .as_slice()
                .iter()
                .map(|selection| selection.head())
                .collect::<Vec<_>>()
        }),
        vec![MultiBufferOffset::new(8), MultiBufferOffset::new(17)]
    );
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), source);
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| {
            editor
                .selections(cx)
                .as_slice()
                .iter()
                .map(|selection| selection.head())
                .collect::<Vec<_>>()
        }),
        vec![MultiBufferOffset::new(4), MultiBufferOffset::new(9)]
    );
    cx.update_entity(&editor, |editor, cx| editor.redo(cx));
    assert_eq!(buffer_text(&buffer, cx), expected);
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| {
            editor
                .selections(cx)
                .as_slice()
                .iter()
                .map(|selection| selection.head())
                .collect::<Vec<_>>()
        }),
        vec![MultiBufferOffset::new(8), MultiBufferOffset::new(17)]
    );
}

#[gpui::test]
fn markdown_nested_empty_list_item_unindents_and_read_only_newline_is_ignored(
    cx: &mut TestAppContext,
) {
    let source = "  - ";
    let (buffer, editor, cx) = editor_with_path(
        cx,
        "README.md",
        source,
        SelectionSet::caret(MultiBufferOffset::new(source.len())),
    );
    cx.run_until_parked();
    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    assert_eq!(buffer_text(&buffer, cx), "- ");
    assert_eq!(
        primary_head(&editor, cx),
        MultiBufferOffset::new("- ".len())
    );
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), source);
    assert_eq!(
        primary_head(&editor, cx),
        MultiBufferOffset::new(source.len())
    );
    cx.update_entity(&editor, |editor, cx| editor.redo(cx));
    assert_eq!(buffer_text(&buffer, cx), "- ");

    let read_only = cx.new(MultiBuffer::empty_read_only);
    cx.update_entity(&read_only, |combined, cx| {
        combined.set_excerpts_for_path(
            vec![ExcerptRange::new(
                buffer.clone(),
                MultiBufferRange::new(MultiBufferOffset::ZERO, MultiBufferOffset::new(2))
                    .unwrap()
                    .into(),
                Vec::new(),
            )],
            cx,
        );
    });
    let read_only_editor = cx.new({
        let read_only = read_only.clone();
        move |cx| {
            let mut editor = Editor::for_multi_buffer(read_only, cx);
            editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(2)), cx);
            editor
        }
    });
    cx.update_entity(&read_only_editor, |editor, cx| editor.insert_newline(cx));
    assert_eq!(buffer_text(&buffer, cx), "- ");
    assert_eq!(
        primary_head(&read_only_editor, cx),
        MultiBufferOffset::new(2)
    );
    cx.update_entity(&read_only_editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), "- ");
    assert_eq!(
        primary_head(&read_only_editor, cx),
        MultiBufferOffset::new(2)
    );
    cx.update_entity(&read_only_editor, |editor, cx| editor.redo(cx));
    assert_eq!(buffer_text(&buffer, cx), "- ");
    assert_eq!(
        primary_head(&read_only_editor, cx),
        MultiBufferOffset::new(2)
    );
}

#[gpui::test]
fn newline_in_markdown_injected_rust_uses_the_injected_scope(cx: &mut TestAppContext) {
    let source = "```rust\n// item\n```\n";
    let at = source.find("item").unwrap() + "item".len();
    let (buffer, editor, cx) = editor_with_path(
        cx,
        "README.md",
        source,
        SelectionSet::caret(MultiBufferOffset::new(at)),
    );
    cx.run_until_parked();
    assert_newline_roundtrip(
        &buffer,
        &editor,
        cx,
        source,
        "```rust\n// item\n// \n```\n",
        at,
        at + 4,
    );
}

#[gpui::test]
fn newline_settings_can_disable_comment_and_list_continuation(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(zcv_settings::SettingsStore::new(
            zcv_settings::UserSettings::default(),
        ));
        cx.update_global::<zcv_settings::SettingsStore, _>(|settings, _| {
            settings
                .set_user_settings(
                    r#"{
                        "extend_comment_on_newline": false,
                        "extend_list_on_newline": false,
                        "auto_indent": "none"
                    }"#,
                )
                .expect("测试设置应能加载");
        });
    });

    let (comment_buffer, comment_editor, cx) = editor_with_rust(
        cx,
        "// item",
        SelectionSet::caret(MultiBufferOffset::new("// item".len())),
    );
    cx.run_until_parked();
    assert_newline_roundtrip(
        &comment_buffer,
        &comment_editor,
        cx,
        "// item",
        "// item\n",
        7,
        8,
    );

    let (list_buffer, list_editor, cx) = editor_with_path(
        cx,
        "README.md",
        "- item",
        SelectionSet::caret(MultiBufferOffset::new("- item".len())),
    );
    cx.run_until_parked();
    assert_newline_roundtrip(&list_buffer, &list_editor, cx, "- item", "- item\n", 6, 7);

    let (code_buffer, code_editor, cx) = editor_with_rust(
        cx,
        "fn main() {",
        SelectionSet::caret(MultiBufferOffset::new("fn main() {".len())),
    );
    cx.run_until_parked();
    assert_newline_roundtrip(
        &code_buffer,
        &code_editor,
        cx,
        "fn main() {",
        "fn main() {\n",
        11,
        12,
    );

    let (pair_buffer, pair_editor, cx) =
        editor_with_rust(cx, "{}", SelectionSet::caret(MultiBufferOffset::new(1)));
    cx.run_until_parked();
    assert_newline_roundtrip(&pair_buffer, &pair_editor, cx, "{}", "{\n}", 1, 2);
}

#[gpui::test]
fn newline_during_ime_composition_skips_comment_continuation(cx: &mut TestAppContext) {
    let (buffer, editor, cx) = editor_with_rust(
        cx,
        "// item",
        SelectionSet::caret(MultiBufferOffset::new("// item".len())),
    );
    cx.update(|window, app| {
        editor.update(app, |editor, cx| {
            editor.replace_and_mark_text_in_range(None, "中", None, window, cx);
        });
    });
    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    assert_eq!(buffer_text(&buffer, cx), "// item中\n");
    assert_eq!(
        primary_head(&editor, cx),
        MultiBufferOffset::new("// item中\n".len())
    );
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), "// item中");
    assert_eq!(
        primary_head(&editor, cx),
        MultiBufferOffset::new("// item中".len())
    );
    cx.update_entity(&editor, |editor, cx| editor.redo(cx));
    assert_eq!(buffer_text(&buffer, cx), "// item中\n");
    assert_eq!(
        primary_head(&editor, cx),
        MultiBufferOffset::new("// item中\n".len())
    );
}

#[gpui::test]
fn newline_on_incomplete_rust_tree_keeps_the_observable_comment_behavior(cx: &mut TestAppContext) {
    let source = "// item(\nfn main(\n";
    let at = source.find('\n').unwrap();
    let (buffer, editor, cx) =
        editor_with_rust(cx, source, SelectionSet::caret(MultiBufferOffset::new(at)));
    assert_newline_roundtrip(
        &buffer,
        &editor,
        cx,
        source,
        "// item(\n// \nfn main(\n",
        at,
        "// item(\n// ".len(),
    );
}

#[gpui::test]
fn tsx_newline_only_bracket_rule_adds_a_blank_inner_line(cx: &mut TestAppContext) {
    let source = "<main></main>";
    let at = "<main>".len();
    let (buffer, editor, cx) = editor_with_path(
        cx,
        "view.tsx",
        source,
        SelectionSet::caret(MultiBufferOffset::new(at)),
    );
    cx.run_until_parked();
    assert_newline_roundtrip(
        &buffer,
        &editor,
        cx,
        source,
        "<main>\n    \n</main>",
        at,
        "<main>\n    ".len(),
    );
}

#[gpui::test]
fn tsx_newline_only_respects_side_whitespace_and_newline_boundary(cx: &mut TestAppContext) {
    for (source, cursor, expected, after) in [
        ("<main>  </main>", 7, "<main> \n    \n </main>", 12),
        ("<main>\n</main>", 7, "<main>\n\n    </main>", 12),
    ] {
        let (buffer, editor, cx) = editor_with_path(
            cx,
            "view.tsx",
            source,
            SelectionSet::caret(MultiBufferOffset::new(cursor)),
        );
        cx.run_until_parked();
        assert_newline_roundtrip(&buffer, &editor, cx, source, expected, cursor, after);
    }
}

#[gpui::test]
fn newline_at_composite_excerpt_boundary_does_not_borrow_next_scope(cx: &mut TestAppContext) {
    let markdown = cx.new(|cx| {
        LanguageBuffer::new(
            Buffer::from_text("- item".to_owned(), BufferConfig::default()).unwrap(),
            Some(PathBuf::from("README.md")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let rust = cx.new(|cx| {
        LanguageBuffer::new(
            Buffer::from_text("fn main() {}".to_owned(), BufferConfig::default()).unwrap(),
            Some(PathBuf::from("z.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        for source in [&markdown, &rust] {
            let len = source.read(cx).text_snapshot().len_bytes();
            buffer.set_excerpts_for_path(
                vec![ExcerptRange::new(
                    source.clone(),
                    MultiBufferRange::new(
                        MultiBufferOffset::ZERO,
                        MultiBufferOffset::new(len.get()),
                    )
                    .unwrap()
                    .into(),
                    Vec::new(),
                )],
                cx,
            );
        }
    });
    let (editor, cx) = cx.add_window_view({
        let combined = combined.clone();
        move |_, cx| {
            let mut editor = Editor::for_multi_buffer(combined, cx);
            editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(6)), cx);
            editor
        }
    });
    cx.run_until_parked();
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(7));
    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| {
            String::from_utf8(editor.display_snapshot(cx).buffer_snapshot().text_bytes())
                .expect("组合快照应保持 UTF-8")
        }),
        "- item\n\nfn main() {}"
    );
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(8));
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&markdown, cx), "- item");
    assert_eq!(buffer_text(&rust, cx), "fn main() {}");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(7));
    cx.update_entity(&editor, |editor, cx| editor.redo(cx));
    assert_eq!(buffer_text(&markdown, cx), "- item");
    assert_eq!(buffer_text(&rust, cx), "\nfn main() {}");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(8));
}

#[gpui::test]
fn script_string_scope_disables_same_quote_in_javascript_and_tsx(cx: &mut TestAppContext) {
    for path in ["main.js", "main.tsx"] {
        let source = "const value = \"ab\";";
        let at = source.find("ab").unwrap() + 1;
        let (buffer, editor, cx) = editor_with_path(
            cx,
            path,
            source,
            SelectionSet::caret(MultiBufferOffset::new(at)),
        );
        cx.run_until_parked();
        type_text(&editor, cx, "\"");
        assert_eq!(
            buffer_text(&buffer, cx),
            "const value = \"a\"b\";",
            "{path}"
        );
        assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(at + 1));
        cx.update_entity(&editor, |editor, cx| editor.undo(cx));
        assert_eq!(buffer_text(&buffer, cx), source);
        assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(at));
    }
}

#[gpui::test]
fn ime_preedit_quote_does_not_create_an_autoclose_pair(cx: &mut TestAppContext) {
    let source = "let x = ;";
    let at = source.find(';').unwrap();
    let (buffer, editor, cx) =
        editor_with_rust(cx, source, SelectionSet::caret(MultiBufferOffset::new(at)));
    cx.update(|window, app| {
        editor.update(app, |editor, cx| {
            editor.replace_and_mark_text_in_range(None, "\"", None, window, cx);
            editor.unmark_text(window, cx);
        });
    });
    assert_eq!(buffer_text(&buffer, cx), "let x = \";");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(at + 1));
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), source);
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(at));
}

#[gpui::test]
fn typing_open_bracket_inserts_matching_close_and_manual_close_skips_it(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_with_rust(cx, "ab", SelectionSet::caret(MultiBufferOffset::new(2)));

    type_text(&editor, cx, "(");
    assert_eq!(buffer_text(&buffer, cx), "ab()");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(3));

    // 手快输入闭合符：跳过自动补全的 `)`，不重复插入。
    type_text(&editor, cx, ")");
    assert_eq!(buffer_text(&buffer, cx), "ab()");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(4));
}

#[gpui::test]
fn typing_inside_pair_keeps_closing_bracket_tracked_for_skip(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_with_rust(cx, "ab", SelectionSet::caret(MultiBufferOffset::new(2)));

    type_text(&editor, cx, "(");
    type_text(&editor, cx, "x");
    assert_eq!(buffer_text(&buffer, cx), "ab(x)");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(4));

    type_text(&editor, cx, ")");
    assert_eq!(buffer_text(&buffer, cx), "ab(x)");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(5));
}

#[gpui::test]
fn nested_pairs_skip_innermost_first(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_with_rust(cx, "ab", SelectionSet::caret(MultiBufferOffset::new(2)));

    type_text(&editor, cx, "(");
    type_text(&editor, cx, "(");
    assert_eq!(buffer_text(&buffer, cx), "ab(())");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(4));

    type_text(&editor, cx, ")");
    type_text(&editor, cx, ")");
    assert_eq!(buffer_text(&buffer, cx), "ab(())");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(6));
}

#[gpui::test]
fn quote_after_word_character_does_not_autoclose(cx: &mut TestAppContext) {
    // 引号类配对前是词字符时不自动闭合，避免打断单词末尾的引号输入。
    let (buffer, editor, cx) =
        editor_with_rust(cx, "ab", SelectionSet::caret(MultiBufferOffset::new(2)));

    type_text(&editor, cx, "'");
    assert_eq!(buffer_text(&buffer, cx), "ab'");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(3));
}

#[gpui::test]
fn quote_after_whitespace_autocloses(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_with_rust(cx, "a ", SelectionSet::caret(MultiBufferOffset::new(2)));

    type_text(&editor, cx, "\"");
    assert_eq!(buffer_text(&buffer, cx), "a \"\"");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(3));
}

#[gpui::test]
fn open_bracket_before_identifier_does_not_autoclose(cx: &mut TestAppContext) {
    // 后续检查：光标后是标识符时不自动闭合。
    let (buffer, editor, cx) =
        editor_with_rust(cx, "ab", SelectionSet::caret(MultiBufferOffset::new(1)));

    type_text(&editor, cx, "(");
    assert_eq!(buffer_text(&buffer, cx), "a(b");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(2));
}

#[gpui::test]
fn typing_open_bracket_surrounds_selection(cx: &mut TestAppContext) {
    let selections = SelectionSet::new(vec![Selection::new(
        MultiBufferOffset::new(0),
        MultiBufferOffset::new(3),
    )]);
    let (buffer, editor, cx) = editor_with_rust(cx, "abc", selections);

    type_text(&editor, cx, "(");
    assert_eq!(buffer_text(&buffer, cx), "(abc)");
    // 编辑后选区覆盖包裹后的文本。
    cx.read_entity(&editor, |editor, cx| {
        let range = editor.selections(cx).primary().range();
        assert_eq!(range.start(), MultiBufferOffset::new(1));
        assert_eq!(range.end(), MultiBufferOffset::new(4));
    });
}

#[gpui::test]
fn backspace_in_empty_pair_deletes_the_whole_pair(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_with_rust(cx, "ab", SelectionSet::caret(MultiBufferOffset::new(2)));

    type_text(&editor, cx, "(");
    backspace(&editor, cx);
    assert_eq!(buffer_text(&buffer, cx), "ab");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(2));
}

#[gpui::test]
fn backspace_deletes_content_then_pair_in_two_steps(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_with_rust(cx, "ab", SelectionSet::caret(MultiBufferOffset::new(2)));

    type_text(&editor, cx, "(");
    type_text(&editor, cx, "x");
    backspace(&editor, cx);
    // 配对内有内容时退格先删内容，区域随编辑收缩回空配对。
    assert_eq!(buffer_text(&buffer, cx), "ab()");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(3));

    backspace(&editor, cx);
    assert_eq!(buffer_text(&buffer, cx), "ab");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(2));
}

#[gpui::test]
fn editor_without_language_inserts_plain_text(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_without_language(cx, "ab", SelectionSet::caret(MultiBufferOffset::new(2)));

    type_text(&editor, cx, "(");
    assert_eq!(buffer_text(&buffer, cx), "ab(");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(3));
}

#[gpui::test]
fn undo_keeps_autoclose_region_valid(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_with_rust(cx, "ab", SelectionSet::caret(MultiBufferOffset::new(2)));

    type_text(&editor, cx, "(");
    type_text(&editor, cx, "x");
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), "ab()");

    // 撤销后区域随回放收缩回空配对，手动闭合符仍被跳过。
    type_text(&editor, cx, ")");
    assert_eq!(buffer_text(&buffer, cx), "ab()");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(4));
}

#[gpui::test]
fn multi_cursor_autocloses_each_selection(cx: &mut TestAppContext) {
    // 两个光标后都允许自动闭合（行尾与前导空白）。
    let selections = SelectionSet::new_with_primary(
        vec![
            Selection::caret(MultiBufferOffset::new(2)),
            Selection::caret(MultiBufferOffset::new(3)),
        ],
        0,
    );
    let (buffer, editor, cx) = editor_with_rust(cx, "ab ", selections);

    type_text(&editor, cx, "(");
    assert_eq!(buffer_text(&buffer, cx), "ab() ()");
    cx.read_entity(&editor, |editor, cx| {
        let heads: Vec<_> = editor
            .selections(cx)
            .as_slice()
            .iter()
            .map(|selection| selection.head())
            .collect();
        assert_eq!(
            heads,
            vec![MultiBufferOffset::new(3), MultiBufferOffset::new(6)]
        );
    });
}

#[gpui::test]
fn newline_inside_pair_inserts_extra_blank_line(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_with_rust(cx, "ab", SelectionSet::caret(MultiBufferOffset::new(2)));

    type_text(&editor, cx, "{");
    assert_eq!(buffer_text(&buffer, cx), "ab{}");
    cx.run_until_parked();
    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    // 光标在 `{` 与自动补全的 `}` 之间：光标行多一层缩进，闭合符前补基准缩进空行。
    assert_eq!(buffer_text(&buffer, cx), "ab{\n    \n}");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(8));
}

#[gpui::test]
fn newline_inside_quote_pair_does_not_add_extra_line(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_with_rust(cx, "a ", SelectionSet::caret(MultiBufferOffset::new(2)));

    type_text(&editor, cx, "\"");
    assert_eq!(buffer_text(&buffer, cx), "a \"\"");
    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    // 引号对未声明 newline：只插入普通换行。
    assert_eq!(buffer_text(&buffer, cx), "a \"\n\"");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(4));
}

#[gpui::test]
fn newline_inside_handwritten_pair_adds_extra_line(cx: &mut TestAppContext) {
    // 文本判断：手写的括号对同样触发。
    let (buffer, editor, cx) =
        editor_with_rust(cx, "ab{}", SelectionSet::caret(MultiBufferOffset::new(3)));

    cx.run_until_parked();
    cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
    // 文本判断：手写的括号对同样触发。
    assert_eq!(buffer_text(&buffer, cx), "ab{\n    \n}");
    assert_eq!(primary_head(&editor, cx), MultiBufferOffset::new(8));
}
#[gpui::test]
fn autoclose_regions_stay_bounded_and_drop_invalid_entries(cx: &mut TestAppContext) {
    let (buffer, editor, cx) =
        editor_with_rust(cx, "ab", SelectionSet::caret(MultiBufferOffset::new(2)));

    // 连续打开 4 层嵌套：每层一个存活区域，全部与光标相交。
    for _ in 0..4 {
        type_text(&editor, cx, "(");
    }
    assert_eq!(buffer_text(&buffer, cx), "ab(((())))");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.autoclose_regions.len(),
            4,
            "每层未闭合配对都应保留一个存活区域"
        );
    });

    // 逐层闭合：每闭合一层，最内层区域立即失效并被移除，剩余区域保留。
    for expected in (0..4).rev() {
        type_text(&editor, cx, ")");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.autoclose_regions.len(),
                expected,
                "闭合后自动闭合区域数量必须收敛到 {expected}"
            );
        });
    }
    assert_eq!(buffer_text(&buffer, cx), "ab(((())))");

    // 继续输入多对括号，区域数保持有界而不是只增不减。
    for _ in 0..16 {
        type_text(&editor, cx, "(");
        type_text(&editor, cx, ")");
    }
    cx.read_entity(&editor, |editor, _| {
        assert!(
            editor.autoclose_regions.len() <= 1,
            "全部闭合后自动闭合区域必须有界，实际 {}",
            editor.autoclose_regions.len()
        );
    });
}
