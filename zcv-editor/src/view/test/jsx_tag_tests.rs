//! JSX/TSX 标签自动闭合行为测试。
//!
//! 每个样例同时校验输入后的文本与光标；撤销、重做、重复触发与手动闭合另列专测。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::path::PathBuf;

use gpui::{AppContext, EntityInputHandler, TestAppContext, VisualTestContext};
use zcv_actions::Backspace;
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{ExcerptRange, MultiBuffer};
use zcv_text::{Buffer, BufferConfig};

use super::Editor;
use crate::selection::{Selection, SelectionSet};

/// 用例中的光标标记。
const CARET: char = 'ˇ';

/// 解析带光标标记的文本，返回去掉标记后的文本与各光标偏移。
pub(super) fn parse_state(marked: &str) -> (String, Vec<MultiBufferOffset>) {
    let mut text = String::new();
    let mut carets = Vec::new();
    for character in marked.chars() {
        if character == CARET {
            carets.push(MultiBufferOffset::new(text.len()));
        } else {
            text.push(character);
        }
    }
    (text, carets)
}

pub(super) fn selection_set(carets: &[MultiBufferOffset]) -> SelectionSet {
    SelectionSet::new_with_primary(
        carets
            .iter()
            .map(|offset| Selection::caret(*offset))
            .collect(),
        0,
    )
}

pub(super) fn editor_with_state<'a>(
    cx: &'a mut TestAppContext,
    path: &'static str,
    marked: &str,
) -> (
    gpui::Entity<LanguageBuffer>,
    gpui::Entity<Editor>,
    &'a mut VisualTestContext,
) {
    let (text, carets) = parse_state(marked);
    let buffer = Buffer::from_text(text, BufferConfig::default()).expect("测试 Buffer 应能创建");
    let language_buffer = cx.new(move |cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from(path)),
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    let editor = cx.add_window_view({
        let language_buffer = language_buffer.clone();
        let selections = selection_set(&carets);
        move |_, cx| {
            let mut editor = Editor::for_language_buffer(language_buffer, cx);
            editor.set_selections(selections, cx);
            editor
        }
    });
    (language_buffer, editor.0, editor.1)
}

pub(super) fn buffer_text(buffer: &gpui::Entity<LanguageBuffer>, cx: &VisualTestContext) -> String {
    cx.read_entity(buffer, |language_buffer, _| {
        let snapshot = language_buffer.text_snapshot();
        snapshot
            .slice_byte_range(MultiBufferOffset::ZERO.into(), snapshot.len_bytes())
            .expect("完整测试范围应可读取")
            .as_str()
            .to_string()
    })
}

pub(super) fn state_text(editor: &gpui::Entity<Editor>, cx: &VisualTestContext) -> String {
    cx.read_entity(editor, |editor, cx| {
        let snapshot = editor.display_snapshot(cx).buffer_snapshot().clone();
        let selections = editor.selections(cx);
        let mut carets = vec![false; snapshot.len_bytes().get() + 1];
        for selection in selections.as_slice() {
            if selection.is_caret() {
                let offset = selection.head().get();
                if offset < carets.len() {
                    carets[offset] = true;
                }
            }
        }
        let text = snapshot
            .text_for_range(
                zcv_multi_buffer::MultiBufferRange::new(
                    MultiBufferOffset::ZERO,
                    snapshot.len_bytes(),
                )
                .expect("全文范围必须合法"),
            )
            .expect("全文文本应可读取");
        let mut marked = String::new();
        for (offset, chunk) in text.char_indices() {
            if carets[offset] {
                marked.push(CARET);
            }
            marked.push(chunk);
        }
        if carets[text.len()] {
            marked.push(CARET);
        }
        marked
    })
}

pub(super) fn type_text(editor: &gpui::Entity<Editor>, cx: &mut VisualTestContext, text: &str) {
    cx.update_entity(editor, |editor, cx| {
        editor.replace_text(None, text, cx);
    });
}

pub(super) fn backspace(editor: &gpui::Entity<Editor>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.handle_backspace(&Backspace, window, cx);
        });
    });
}

/// 输入 `>` 并比较带光标标记的最终状态。
fn check(cx: &mut TestAppContext, before_marked: &str, after_marked: &str) {
    let (buffer, editor, cx) = editor_with_state(cx, "test.tsx", before_marked);
    cx.run_until_parked();
    type_text(&editor, cx, ">");
    cx.run_until_parked();
    let expected = after_marked.replace(CARET, "");
    assert_eq!(
        buffer_text(&buffer, cx),
        expected,
        "文本不一致：{before_marked}"
    );
    assert_eq!(
        state_text(&editor, cx),
        after_marked,
        "光标不一致：{before_marked}"
    );
}

#[gpui::test]
fn tsx_tag_autoclose_matches_zed_cases(cx: &mut TestAppContext) {
    check(cx, "<divˇ", "<div>ˇ</div>");
    check(cx, "<div><divˇ</div>", "<div><div>ˇ</div></div>");
    check(cx, "<div><divˇ</div></div>", "<div><div>ˇ</div></div>");
    check(cx, "</divˇ", "</div>ˇ");
    check(cx, "<div attr={</div>}ˇ", "<div attr={</div>}>ˇ</div>");
    check(
        cx,
        "<div><divˇ{</div>}</div>",
        "<div><div>ˇ</div>{</div>}</div>",
    );
    check(cx, "<div attr={1 ˇ", "<div attr={1 >ˇ");
    check(
        cx,
        "<div><divˇ</div></span>",
        "<div><div>ˇ</div></div></span>",
    );
    check(cx, "<div>{<divˇ}</div>", "<div>{<div>ˇ</div>}</div>");
    check(cx, "<div>{<divˇ</div>}</div>", "<div>{<div>ˇ</div>}</div>");
    check(cx, "<ˇ", "<>ˇ</>");
    check(
        cx,
        "<Component<T> attr={boolean_value}ˇ",
        "<Component<T> attr={boolean_value}>ˇ</Component>",
    );
    check(cx, "<!DOCTYPE htmlˇ", "<!DOCTYPE html>ˇ");
    check(cx, "<!-- comment --ˇ", "<!-- comment -->ˇ");
    check(cx, "<Component.Fooˇ", "<Component.Foo>ˇ</Component.Foo>");
    check(cx, "<divˇfoobar", "<div>ˇ</div>foobar");
}

#[gpui::test]
fn tsx_tag_autoclose_supports_multiple_cursors(cx: &mut TestAppContext) {
    let (buffer, editor, cx) = editor_with_state(
        cx,
        "test.tsx",
        "<divˇ
<spanˇ",
    );
    cx.run_until_parked();
    type_text(&editor, cx, ">");
    cx.run_until_parked();
    assert_eq!(
        buffer_text(&buffer, cx),
        "<div></div>
<span></span>"
    );
}

#[gpui::test]
fn tsx_tag_autoclose_commits_in_one_undo_step(cx: &mut TestAppContext) {
    let (buffer, editor, cx) = editor_with_state(cx, "test.tsx", "<divˇ");
    cx.run_until_parked();
    type_text(&editor, cx, ">");
    assert_eq!(buffer_text(&buffer, cx), "<div></div>");
    cx.update_entity(&editor, |editor, cx| editor.undo(cx));
    assert_eq!(buffer_text(&buffer, cx), "<div");
    assert_eq!(state_text(&editor, cx), "<divˇ");
    cx.update_entity(&editor, |editor, cx| editor.redo(cx));
    assert_eq!(buffer_text(&buffer, cx), "<div></div>");
    assert_eq!(state_text(&editor, cx), "<div>ˇ</div>");
}

#[gpui::test]
fn tsx_tag_autoclose_does_not_duplicate_on_repeat_input(cx: &mut TestAppContext) {
    let (buffer, editor, cx) = editor_with_state(cx, "test.tsx", "<divˇ");
    cx.run_until_parked();
    type_text(&editor, cx, ">");
    assert_eq!(buffer_text(&buffer, cx), "<div></div>");
    // 光标停在 `>` 与闭合标签之间，再输入 `>` 不应再生成闭合标签。
    type_text(&editor, cx, ">");
    assert_eq!(buffer_text(&buffer, cx), "<div>></div>");
}

#[gpui::test]
fn tsx_tag_autoclose_skips_manual_closing_tag(cx: &mut TestAppContext) {
    let (buffer, editor, cx) = editor_with_state(cx, "test.tsx", "<div>ˇ");
    cx.run_until_parked();
    // 手动输入闭合标签时，最后的 `>` 属于闭合标签，不应再补全。
    type_text(&editor, cx, "</div>");
    assert_eq!(buffer_text(&buffer, cx), "<div></div>");
}

#[gpui::test]
fn tsx_tag_autoclose_only_applies_to_jsx_and_tsx(cx: &mut TestAppContext) {
    let (buffer, editor, cx) = editor_with_state(cx, "test.ts", "a = bˇ");
    cx.run_until_parked();
    type_text(&editor, cx, ">");
    assert_eq!(buffer_text(&buffer, cx), "a = b>");
    assert_eq!(state_text(&editor, cx), "a = b>ˇ");
}

#[gpui::test]
fn tsx_tag_autoclose_does_not_fire_for_comparison(cx: &mut TestAppContext) {
    let (buffer, editor, cx) = editor_with_state(cx, "test.tsx", "const n = aˇ");
    cx.run_until_parked();
    type_text(&editor, cx, ">");
    assert_eq!(buffer_text(&buffer, cx), "const n = a>");
}
#[gpui::test]
fn tsx_tag_autoclose_during_ime_composition_is_skipped(cx: &mut TestAppContext) {
    let (buffer, editor, cx) = editor_with_state(cx, "test.tsx", "<divˇ");
    cx.run_until_parked();
    cx.update(|window, app| {
        editor.update(app, |editor, cx| {
            // 组合输入期间不触发自动补全；输入的 `>` 走普通组合事务。
            editor.replace_and_mark_text_in_range(None, ">", None, window, cx);
            editor.unmark_text(window, cx);
        });
    });
    assert_eq!(buffer_text(&buffer, cx), "<div>");
}

#[gpui::test]
fn tsx_tag_autoclose_is_rejected_in_read_only_excerpt(cx: &mut TestAppContext) {
    let (buffer, _editor, cx) = editor_with_state(cx, "test.tsx", "<divˇ");
    cx.run_until_parked();
    let combined = cx.new(MultiBuffer::empty_read_only);
    cx.update_entity(&combined, |combined, cx| {
        combined.set_excerpts_for_path(
            vec![ExcerptRange::new(
                buffer.clone(),
                MultiBufferRange::new(MultiBufferOffset::ZERO, MultiBufferOffset::new(4))
                    .expect("excerpt 范围必须合法")
                    .into(),
                Vec::new(),
            )],
            cx,
        );
    });
    let read_only_editor = cx.new({
        let combined = combined.clone();
        move |cx| {
            let mut editor = Editor::for_multi_buffer(combined, cx);
            editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(4)), cx);
            editor
        }
    });
    cx.run_until_parked();
    cx.update_entity(&read_only_editor, |editor, cx| {
        editor.replace_text(None, ">", cx);
    });
    assert_eq!(buffer_text(&buffer, cx), "<div");
}

#[gpui::test]
fn tsx_tag_autoclose_applies_per_excerpt_in_composite_document(cx: &mut TestAppContext) {
    let registry = std::sync::Arc::new(LanguageRegistry::new());
    let first = cx.new({
        let registry = registry.clone();
        move |cx| {
            LanguageBuffer::new(
                Buffer::from_text("<div ".to_owned(), BufferConfig::default())
                    .expect("测试 Buffer 应能创建"),
                Some(PathBuf::from("a.tsx")),
                registry,
                cx,
            )
        }
    });
    let second = cx.new({
        let registry = registry.clone();
        move |cx| {
            LanguageBuffer::new(
                Buffer::from_text("<span ".to_owned(), BufferConfig::default())
                    .expect("测试 Buffer 应能创建"),
                Some(PathBuf::from("b.tsx")),
                registry,
                cx,
            )
        }
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(
            vec![ExcerptRange::new(
                first.clone(),
                MultiBufferRange::new(MultiBufferOffset::ZERO, MultiBufferOffset::new(5))
                    .expect("excerpt 范围必须合法")
                    .into(),
                Vec::new(),
            )],
            cx,
        );
        buffer.set_excerpts_for_path(
            vec![ExcerptRange::new(
                second.clone(),
                MultiBufferRange::new(MultiBufferOffset::ZERO, MultiBufferOffset::new(6))
                    .expect("excerpt 范围必须合法")
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
            // 组合输出坐标：第一段 0..5，分隔换行在 5，第二段自 6 起；光标停在标签名末尾。
            editor.set_selections(
                SelectionSet::new(vec![
                    Selection::caret(MultiBufferOffset::new(4)),
                    Selection::caret(MultiBufferOffset::new(11)),
                ]),
                cx,
            );
            editor
        }
    });
    cx.run_until_parked();
    type_text(&editor, cx, ">");
    cx.run_until_parked();
    assert_eq!(buffer_text(&first, cx), "<div></div> ");
    assert_eq!(buffer_text(&second, cx), "<span></span> ");
}

#[gpui::test]
fn tsx_tag_autoclose_backspace_removes_only_typed_characters(cx: &mut TestAppContext) {
    let (buffer, editor, cx) = editor_with_state(cx, "test.tsx", "<divˇ");
    cx.run_until_parked();
    type_text(&editor, cx, ">");
    assert_eq!(buffer_text(&buffer, cx), "<div></div>");
    // 光标位于两标签之间，退格删除刚输入的 `>`，自动补全标签一并失效。
    backspace(&editor, cx);
    assert_eq!(buffer_text(&buffer, cx), "<div</div>");
}
