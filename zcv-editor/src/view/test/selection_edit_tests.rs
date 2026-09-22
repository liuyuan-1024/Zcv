//! Editor 选区编辑行为测试。

use zcv_multi_buffer::MultiBufferOffset;

use std::path::PathBuf;

use gpui::{AppContext, TestAppContext};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{ExcerptRange, MultiBuffer};
use zcv_text::{Buffer, BufferConfig};

use super::Editor;
use crate::selection::{Selection, SelectionSet};

fn editor_with_text(
    cx: &mut TestAppContext,
    text: &str,
    selections: SelectionSet,
) -> (gpui::Entity<LanguageBuffer>, gpui::Entity<Editor>) {
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
    let editor = cx.new({
        let language_buffer = language_buffer.clone();
        move |cx| {
            let mut editor = Editor::for_language_buffer(language_buffer, cx);
            editor.set_selections(selections, cx);
            editor
        }
    });
    (language_buffer, editor)
}

fn buffer_text(buffer: &gpui::Entity<LanguageBuffer>, cx: &TestAppContext) -> String {
    cx.read_entity(buffer, |language_buffer, _| {
        let snapshot = language_buffer.text_snapshot();
        snapshot
            .slice_byte_range(MultiBufferOffset::ZERO.into(), snapshot.len_bytes())
            .expect("完整测试范围应可读取")
            .as_str()
            .to_string()
    })
}

#[gpui::test]
fn rename_local_at_replaces_only_the_resolved_binding(cx: &mut TestAppContext) {
    let source = "fn main(value: i32) { let result = value; return result; }\n";
    let buffer =
        Buffer::from_text(source.to_string(), BufferConfig::default()).expect("测试 Buffer 应创建");
    let language_buffer = cx.new(move |cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("rename.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let editor = cx.new({
        let language_buffer = language_buffer.clone();
        move |cx| Editor::for_language_buffer(language_buffer, cx)
    });
    cx.run_until_parked();

    let result_offset = source.find("result").expect("测试文本应包含局部变量");
    cx.update_entity(&editor, |editor, cx| {
        editor
            .rename_local_at(MultiBufferOffset::new(result_offset), "answer", cx)
            .expect("已解析的局部绑定应可重命名");
    });

    assert_eq!(
        buffer_text(&language_buffer, cx),
        "fn main(value: i32) { let answer = value; return answer; }\n"
    );
}

#[gpui::test]
fn rename_local_at_rejects_ambiguous_binding(cx: &mut TestAppContext) {
    let source = "fn main() { let value = 1; let value = 2; value; }\n";
    let buffer =
        Buffer::from_text(source.to_string(), BufferConfig::default()).expect("测试 Buffer 应创建");
    let language_buffer = cx.new(move |cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("ambiguous.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let editor = cx.new({
        let language_buffer = language_buffer.clone();
        move |cx| Editor::for_language_buffer(language_buffer, cx)
    });
    cx.run_until_parked();

    let value_offset = source.find("value").expect("测试文本应包含重复绑定");
    let result = cx.update_entity(&editor, |editor, cx| {
        editor.rename_local_at(MultiBufferOffset::new(value_offset), "answer", cx)
    });

    assert!(result.is_err(), "歧义绑定不能执行批量重命名");
    assert_eq!(buffer_text(&language_buffer, cx), source);
}

#[gpui::test]
fn rename_local_at_rejects_unresolved_reference(cx: &mut TestAppContext) {
    let source = "fn main() { let value = missing; value; }\n";
    let buffer =
        Buffer::from_text(source.to_string(), BufferConfig::default()).expect("测试 Buffer 应创建");
    let language_buffer = cx.new(move |cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("unresolved.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let editor = cx.new({
        let language_buffer = language_buffer.clone();
        move |cx| Editor::for_language_buffer(language_buffer, cx)
    });
    cx.run_until_parked();

    let missing_offset = source.find("missing").expect("测试文本应包含未解析引用");
    let result = cx.update_entity(&editor, |editor, cx| {
        editor.rename_local_at(MultiBufferOffset::new(missing_offset), "answer", cx)
    });

    assert!(result.is_err(), "未解析引用不能执行批量重命名");
    assert_eq!(buffer_text(&language_buffer, cx), source);
}

#[gpui::test]
fn indent_and_outdent_are_editor_owned_selection_edits(cx: &mut TestAppContext) {
    let selections = SelectionSet::new(vec![Selection::new(
        MultiBufferOffset::new(0),
        MultiBufferOffset::new(3),
    )]);
    let (buffer, editor) = editor_with_text(cx, "a\nb", selections);

    cx.update_entity(&editor, |editor, cx| editor.indent(cx));
    assert_eq!(buffer_text(&buffer, cx), "    a\n    b");
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx),
            SelectionSet::new(vec![Selection::new(
                MultiBufferOffset::new(4),
                MultiBufferOffset::new(11),
            )]),
            "多行缩进后应保持一个覆盖原内容的选区"
        );
    });

    cx.update_entity(&editor, |editor, cx| editor.outdent(cx));
    assert_eq!(buffer_text(&buffer, cx), "a\nb");
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx),
            SelectionSet::new(vec![Selection::new(
                MultiBufferOffset::ZERO,
                MultiBufferOffset::new(3)
            )]),
            "减少缩进后应恢复原选区"
        );
    });
}

#[gpui::test]
fn caret_indent_uses_display_map_tab_column(cx: &mut TestAppContext) {
    let (buffer, editor) =
        editor_with_text(cx, "\tx", SelectionSet::caret(MultiBufferOffset::new(1)));

    cx.update_entity(&editor, |editor, cx| editor.indent(cx));

    assert_eq!(buffer_text(&buffer, cx), "\t    x");
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.selections(cx).primary().head(),
            MultiBufferOffset::new(5)
        );
    });
}

#[gpui::test]
fn editing_a_later_composite_excerpt_keeps_following_input_in_that_source(cx: &mut TestAppContext) {
    let first = Buffer::from_text("first\n".to_string(), BufferConfig::default())
        .expect("应创建测试 Buffer");
    let second = Buffer::from_text("second\n".to_string(), BufferConfig::default())
        .expect("应创建测试 Buffer");
    let first = cx.new(|cx| {
        LanguageBuffer::new(
            first,
            Some(std::path::PathBuf::from("src/first.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let second = cx.new(|cx| {
        LanguageBuffer::new(
            second,
            Some(std::path::PathBuf::from("src/second.rs")),
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(first, 0..1, cx)], cx);
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(second.clone(), 0..1, cx)], cx);
    });
    let editor = cx.new({
        let combined = combined.clone();
        move |cx| Editor::for_multi_buffer(combined, cx)
    });

    cx.update_entity(&editor, |editor, cx| {
        editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(6)), cx);
        editor.replace_text(None, "A", cx);
        editor.replace_text(None, "B", cx);
    });

    assert_eq!(buffer_text(&second, cx), "ABsecond\n");
}
