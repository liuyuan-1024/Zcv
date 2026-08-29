//! Editor 选区编辑行为测试。

use gpui::{AppContext, TestAppContext};
use zcv_language::LanguageBuffer;
use zcv_text::{Buffer, BufferConfig, ByteOffset};

use super::Editor;
use crate::selection::{Selection, SelectionSet};

fn editor_with_text(
    cx: &mut TestAppContext,
    text: &str,
    selections: SelectionSet,
) -> (gpui::Entity<Buffer>, gpui::Entity<Editor>) {
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_string(), BufferConfig::default()).expect("测试 Buffer 应能创建")
    });
    let language_buffer = cx.new({
        let buffer = buffer.clone();
        move |cx| LanguageBuffer::new(buffer, None, cx)
    });
    let editor = cx.new({
        let language_buffer = language_buffer.clone();
        move |cx| {
            let mut editor = Editor::for_language_buffer(language_buffer, cx);
            editor.set_selections(selections);
            editor
        }
    });
    (buffer, editor)
}

fn buffer_text(buffer: &gpui::Entity<Buffer>, cx: &TestAppContext) -> String {
    cx.read_entity(buffer, |buffer, _| {
        buffer
            .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
            .expect("完整测试范围应可读取")
            .as_str()
            .to_string()
    })
}

#[gpui::test]
fn indent_and_outdent_are_editor_owned_selection_edits(cx: &mut TestAppContext) {
    let selections =
        SelectionSet::new(vec![Selection::new(ByteOffset::new(0), ByteOffset::new(3))]);
    let (buffer, editor) = editor_with_text(cx, "a\nb", selections);

    cx.update_entity(&editor, |editor, cx| editor.indent(cx));
    assert_eq!(buffer_text(&buffer, cx), "    a\n    b");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections(),
            SelectionSet::new(vec![Selection::new(
                ByteOffset::new(4),
                ByteOffset::new(11),
            )]),
            "多行缩进后应保持一个覆盖原内容的选区"
        );
    });

    cx.update_entity(&editor, |editor, cx| editor.outdent(cx));
    assert_eq!(buffer_text(&buffer, cx), "a\nb");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections(),
            SelectionSet::new(vec![Selection::new(ByteOffset::ZERO, ByteOffset::new(3))]),
            "减少缩进后应恢复原选区"
        );
    });
}

#[gpui::test]
fn caret_indent_uses_display_map_tab_column(cx: &mut TestAppContext) {
    let (buffer, editor) = editor_with_text(cx, "\tx", SelectionSet::caret(ByteOffset::new(1)));

    cx.update_entity(&editor, |editor, cx| editor.indent(cx));

    assert_eq!(buffer_text(&buffer, cx), "\t    x");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections().primary().head(), ByteOffset::new(5));
    });
}
