//! M6C 机器契约：锁定 IME composition 的 start/update/commit/cancel、preedit 和 selection 恢复语义。
//!
//! 本文件只验证引擎状态机对外行为，不测试真实系统 IME 事件或 GPUI 输入法桥接。

use zom_engine::{Buffer, BufferConfig, CharOffset, CompositionSelection, Selection, SelectionSet};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn caret(offset: usize) -> SelectionSet {
    SelectionSet::caret(CharOffset::new(offset))
}

fn selection(anchor: usize, head: usize) -> SelectionSet {
    SelectionSet::new(vec![Selection::new(
        CharOffset::new(anchor),
        CharOffset::new(head),
    )])
}

#[test]
fn composition_update_inserts_preedit_without_history() {
    let mut buffer = buffer("hello");
    buffer.set_selection(caret(5)).unwrap();

    let state = buffer.start_composition().unwrap();
    assert!(state.preedit_text().is_empty());
    assert!(buffer.is_composing());

    let result = buffer.update_composition("世", None).unwrap();
    assert!(result.is_some());
    assert_eq!(buffer.text().as_ref(), "hello世");
    assert_eq!(buffer.composition().unwrap().preedit_text(), "世");
    assert_eq!(
        buffer.composition().unwrap().range().start(),
        CharOffset::new(5)
    );
    assert_eq!(
        buffer.composition().unwrap().range().end(),
        CharOffset::new(6)
    );
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(6));
    assert!(!buffer.can_undo());
}

#[test]
fn composition_commit_records_one_undo_step_from_original_to_final_text() {
    let mut buffer = buffer("hello");
    buffer.set_selection(caret(5)).unwrap();

    buffer.start_composition().unwrap();
    buffer.update_composition("世", None).unwrap();
    buffer.update_composition("世界", None).unwrap();
    buffer.commit_composition("世界").unwrap();

    assert!(!buffer.is_composing());
    assert_eq!(buffer.text().as_ref(), "hello世界");
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(7));
    assert!(buffer.can_undo());

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer.text().as_ref(), "hello");
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(5));

    buffer.redo().unwrap().unwrap();
    assert_eq!(buffer.text().as_ref(), "hello世界");
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(7));
}

#[test]
fn composition_cancel_restores_original_text_selection_and_clean_state() {
    let mut buffer = buffer("abc");
    buffer.mark_saved();
    assert!(!buffer.is_dirty());

    buffer.set_selection(caret(1)).unwrap();
    buffer.start_composition().unwrap();
    buffer.update_composition("中", None).unwrap();

    assert_eq!(buffer.text().as_ref(), "a中bc");
    assert!(buffer.is_dirty());
    assert!(!buffer.can_undo());

    let result = buffer.cancel_composition().unwrap();
    assert!(result.is_some());
    assert!(!buffer.is_composing());
    assert_eq!(buffer.text().as_ref(), "abc");
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(1));
    assert!(!buffer.can_undo());
    assert!(!buffer.is_dirty());
}

#[test]
fn composition_replaces_selected_range_and_undo_restores_that_selection() {
    let mut buffer = buffer("abc def");
    buffer.set_selection(selection(4, 7)).unwrap();

    buffer.start_composition().unwrap();
    buffer.update_composition("世", None).unwrap();
    assert_eq!(buffer.text().as_ref(), "abc 世");

    buffer.commit_composition("世界").unwrap();
    assert_eq!(buffer.text().as_ref(), "abc 世界");
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(6));

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer.text().as_ref(), "abc def");
    assert_eq!(buffer.selection().primary().anchor(), CharOffset::new(4));
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(7));
}

#[test]
fn composition_update_tracks_relative_composition_selection() {
    let mut buffer = buffer("ab");
    buffer.set_selection(caret(1)).unwrap();
    buffer.start_composition().unwrap();

    buffer
        .update_composition(
            "nihao",
            Some(CompositionSelection::new(
                CharOffset::new(1),
                CharOffset::new(3),
            )),
        )
        .unwrap();

    assert_eq!(buffer.text().as_ref(), "anihaob");
    assert_eq!(
        buffer.composition().unwrap().selection().anchor(),
        CharOffset::new(2)
    );
    assert_eq!(
        buffer.composition().unwrap().selection().head(),
        CharOffset::new(4)
    );
    assert_eq!(buffer.selection().primary().anchor(), CharOffset::new(2));
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(4));
}

#[test]
fn composition_update_rejects_relative_selection_inside_grapheme_cluster() {
    let mut buffer = buffer("ab");
    buffer.set_selection(caret(1)).unwrap();
    buffer.start_composition().unwrap();

    let result = buffer.update_composition(
        "e\u{0301}",
        Some(CompositionSelection::caret(CharOffset::new(1))),
    );

    assert!(result.is_err());
    assert!(buffer.is_composing());
    assert_eq!(buffer.text().as_ref(), "ab");
}

#[test]
fn composition_degrades_multi_cursor_to_primary_selection() {
    let mut buffer = buffer("abc");
    buffer
        .set_selection(SelectionSet::new_with_primary(
            vec![
                Selection::caret(CharOffset::new(0)),
                Selection::caret(CharOffset::new(3)),
            ],
            1,
        ))
        .unwrap();

    buffer.start_composition().unwrap();

    assert_eq!(buffer.selection().len(), 1);
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(3));

    buffer.update_composition("中", None).unwrap();
    assert_eq!(buffer.text().as_ref(), "abc中");
}

#[test]
fn ordinary_text_edit_cancels_active_composition_before_editing() {
    let mut buffer = buffer("abc");
    buffer.set_selection(caret(3)).unwrap();
    buffer.start_composition().unwrap();
    buffer.update_composition("中", None).unwrap();
    assert_eq!(buffer.text().as_ref(), "abc中");

    buffer.insert(CharOffset::ZERO, "!").unwrap();

    assert!(!buffer.is_composing());
    assert_eq!(buffer.text().as_ref(), "!abc");
    assert!(buffer.can_undo());

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer.text().as_ref(), "abc");
}

#[test]
fn composition_commit_without_active_session_behaves_like_single_text_transaction() {
    let mut buffer = buffer("abc");
    buffer.set_selection(caret(0)).unwrap();

    buffer.commit_composition("你").unwrap();

    assert_eq!(buffer.text().as_ref(), "你abc");
    assert_eq!(buffer.selection().primary().head(), CharOffset::new(1));
    assert!(buffer.can_undo());

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer.text().as_ref(), "abc");
}
