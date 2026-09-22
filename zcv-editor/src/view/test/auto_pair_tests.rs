//! 自动闭合配对行为测试：自动补全闭合符、跳过已存在闭合符、包裹选区、退格删除整对与撤销回放。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::path::PathBuf;

use gpui::{AppContext, TestAppContext, VisualTestContext};
use zcv_actions::Backspace;
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{ExcerptRange, MultiBuffer};
use zcv_text::{Buffer, BufferConfig};

use super::Editor;
use crate::selection::{Selection, SelectionSet};

/// 带 Rust 语言的窗口化编辑器：语言在构造时同步识别（见 `LanguageBuffer::new`），自动闭合行为依赖语言提供的配对表。
fn editor_with_rust<'a>(
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
            Some(PathBuf::from("test.rs")),
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
