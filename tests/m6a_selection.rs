//! M6A 机器契约：锁定 Cursor、Selection、SelectionSet、多光标编辑和历史 selection 恢复。
//!
//! 本文件验证 selection 主模型的 public 行为，不测试 word movement、IME composition 或 UI 手感。

use zom_engine::*;

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(CharOffset::new(start), CharOffset::new(end)).unwrap()
}

fn selection(anchor: usize, head: usize) -> Selection {
    Selection::new(CharOffset::new(anchor), CharOffset::new(head))
}

fn caret(offset: usize) -> Selection {
    Selection::caret(CharOffset::new(offset))
}

#[test]
fn cursor_can_roundtrip_to_caret_selection() {
    let cursor = Cursor::new(CharOffset::new(3));
    let selection = cursor.to_selection();

    assert_eq!(cursor.offset(), CharOffset::new(3));
    assert_eq!(selection, caret(3));
    assert_eq!(selection.cursor(), Some(cursor));
}

#[test]
fn non_caret_selection_has_no_cursor() {
    assert_eq!(selection(1, 3).cursor(), None);
}

#[test]
fn selection_preserves_anchor_head_and_exposes_ordered_range() {
    let selection = selection(5, 2);

    assert_eq!(selection.anchor(), CharOffset::new(5));
    assert_eq!(selection.head(), CharOffset::new(2));
    assert!(selection.is_reversed());
    assert_eq!(selection.start(), CharOffset::new(2));
    assert_eq!(selection.end(), CharOffset::new(5));
    assert_eq!(selection.range(), range(2, 5));
}

#[test]
fn selection_can_collapse_to_start_or_end() {
    let sel = selection(5, 2);

    assert_eq!(sel.collapse_to_start(), caret(2));
    assert_eq!(sel.collapse_to_end(), caret(5));
    assert_eq!(sel.with_head(CharOffset::new(9)), selection(5, 9));
}

#[test]
fn empty_selection_set_normalizes_to_document_start_caret() {
    let selections = SelectionSet::new(vec![]);

    assert_eq!(selections.len(), 1);
    assert_eq!(selections.primary_index(), 0);
    assert_eq!(selections.primary(), &caret(0));
    assert_eq!(selections.ranges(), vec![range(0, 0)]);
}

#[test]
fn selection_set_normalizes_sorting_and_merges_overlaps() {
    let selections = SelectionSet::new(vec![
        selection(6, 8),
        caret(3),
        selection(2, 5),
        selection(4, 7),
    ]);

    assert_eq!(selections.ranges(), vec![range(2, 8)]);
}

#[test]
fn selection_set_merges_duplicate_carets() {
    let selections = SelectionSet::new(vec![caret(3), caret(1), caret(3), caret(1)]);

    assert_eq!(selections.ranges(), vec![range(1, 1), range(3, 3)]);
}

#[test]
fn adjacent_non_empty_ranges_are_not_merged_by_default() {
    let selections = SelectionSet::new(vec![selection(1, 3), selection(3, 5)]);

    assert_eq!(selections.ranges(), vec![range(1, 3), range(3, 5)]);
}

#[test]
fn adjacent_non_empty_ranges_can_be_merged_with_explicit_policy() {
    let selections = SelectionSet::new_with_policy(
        vec![selection(1, 3), selection(3, 5)],
        0,
        SelectionMergePolicy::MergeOverlappingOrAdjacent,
    );

    assert_eq!(selections.ranges(), vec![range(1, 5)]);
}

#[test]
fn selection_set_preserves_primary_selection_after_sorting() {
    let selections = SelectionSet::new_with_primary(vec![caret(8), caret(1), caret(5)], 2);

    assert_eq!(
        selections.ranges(),
        vec![range(1, 1), range(5, 5), range(8, 8)]
    );
    assert_eq!(selections.primary_index(), 1);
    assert_eq!(selections.primary(), &caret(5));
}

#[test]
fn buffer_default_selection_is_document_start_caret() {
    let buffer = buffer("abcd");

    assert_eq!(buffer.selection(), &SelectionSet::caret(CharOffset::ZERO));
}

#[test]
fn buffer_stores_m6_selection_set_directly() {
    let mut buffer = buffer("abcd");
    let selections = SelectionSet::new(vec![caret(1), selection(3, 4)]);

    buffer.set_selection(selections.clone()).unwrap();

    assert_eq!(buffer.selection(), &selections);
    assert_eq!(buffer.selection().ranges(), selections.ranges());
}

#[test]
fn set_selection_rejects_out_of_bounds_offsets() {
    let mut buffer = buffer("abc");
    let original = buffer.selection().clone();

    let result = buffer.set_selection(SelectionSet::caret(CharOffset::new(4)));

    assert!(result.is_err());
    assert_eq!(buffer.selection(), &original);
}

#[test]
fn set_selection_rejects_grapheme_middle_offsets() {
    let mut buffer = buffer("ae\u{301}b");
    let original = buffer.selection().clone();

    let result = buffer.set_selection(SelectionSet::caret(CharOffset::new(2)));

    assert!(result.is_err());
    assert_eq!(buffer.selection(), &original);
}

#[test]
fn set_selection_rejects_crlf_middle_offsets() {
    let mut buffer = buffer("a\r\nb");
    let original = buffer.selection().clone();

    let result = buffer.set_selection(SelectionSet::caret(CharOffset::new(2)));

    assert!(result.is_err());
    assert_eq!(buffer.selection(), &original);
}

#[test]
fn multi_cursor_insert_uses_one_transaction_and_updates_carets() {
    let mut buffer = buffer("abcd");
    let selections = SelectionSet::new(vec![caret(1), caret(3)]);

    let result = buffer.insert_at_selections(selections, "X").unwrap();

    assert!(result.is_some());
    assert_eq!(buffer.text().as_ref(), "aXbcXd");
    assert_eq!(buffer.history_status().undo_depth, 1);
    assert_eq!(buffer.selection().ranges(), vec![range(2, 2), range(5, 5)]);
}

#[test]
fn multi_cursor_insert_supports_multibyte_text() {
    let mut buffer = buffer("你a好b");
    let selections = SelectionSet::new(vec![caret(1), caret(3)]);

    buffer.insert_at_selections(selections, "中").unwrap();

    assert_eq!(buffer.text().as_ref(), "你中a好中b");
    assert_eq!(buffer.selection().ranges(), vec![range(2, 2), range(5, 5)]);
}

#[test]
fn multi_selection_replace_merges_overlaps_before_editing() {
    let mut buffer = buffer("abcdef");
    let selections = SelectionSet::new(vec![selection(1, 4), selection(3, 5)]);

    buffer.replace_selections(selections, "X").unwrap();

    assert_eq!(buffer.text().as_ref(), "aXf");
    assert_eq!(buffer.selection().ranges(), vec![range(2, 2)]);
}

#[test]
fn multi_selection_replace_collapses_each_selection_after_replacement() {
    let mut buffer = buffer("abcdef");
    let selections = SelectionSet::new(vec![selection(1, 2), selection(4, 6)]);

    buffer.replace_selections(selections, "XX").unwrap();

    assert_eq!(buffer.text().as_ref(), "aXXcdXX");
    assert_eq!(buffer.selection().ranges(), vec![range(3, 3), range(7, 7)]);
}

#[test]
fn multi_selection_delete_deletes_non_empty_ranges_only() {
    let mut buffer = buffer("abcdef");
    let selections = SelectionSet::new(vec![selection(1, 3), caret(3), selection(4, 6)]);

    buffer.delete_selection_ranges(selections).unwrap();

    assert_eq!(buffer.text().as_ref(), "ad");
    assert_eq!(buffer.selection().ranges(), vec![range(1, 1), range(2, 2)]);
}

#[test]
fn delete_selection_ranges_with_only_carets_is_noop_but_updates_selection() {
    let mut buffer = buffer("abcdef");
    let selections = SelectionSet::new(vec![caret(1), caret(4)]);

    let result = buffer.delete_selection_ranges(selections.clone()).unwrap();

    assert!(result.is_none());
    assert_eq!(buffer.text().as_ref(), "abcdef");
    assert_eq!(buffer.version(), BufferVersion::INITIAL);
    assert_eq!(buffer.history_status().undo_depth, 0);
    assert_eq!(buffer.selection(), &selections);
}

#[test]
fn empty_insert_at_carets_is_noop_but_updates_selection() {
    let mut buffer = buffer("abcdef");
    let selections = SelectionSet::new(vec![caret(2), caret(5)]);

    let result = buffer.insert_at_selections(selections.clone(), "").unwrap();

    assert!(result.is_none());
    assert_eq!(buffer.text().as_ref(), "abcdef");
    assert_eq!(buffer.version(), BufferVersion::INITIAL);
    assert_eq!(buffer.history_status().undo_depth, 0);
    assert_eq!(buffer.selection(), &selections);
}

#[test]
fn delete_backward_is_grapheme_safe() {
    let mut buffer = buffer("ae\u{301}b");
    let selections = SelectionSet::caret(CharOffset::new(3));

    buffer.delete_backward_at_selections(selections).unwrap();

    assert_eq!(buffer.text().as_ref(), "ab");
    assert_eq!(buffer.selection().ranges(), vec![range(1, 1)]);
}

#[test]
fn delete_forward_is_grapheme_safe() {
    let mut buffer = buffer("ae\u{301}b");
    let selections = SelectionSet::caret(CharOffset::new(1));

    buffer.delete_forward_at_selections(selections).unwrap();

    assert_eq!(buffer.text().as_ref(), "ab");
    assert_eq!(buffer.selection().ranges(), vec![range(1, 1)]);
}

#[test]
fn delete_backward_at_document_start_is_noop() {
    let mut buffer = buffer("abc");
    let selections = SelectionSet::new(vec![caret(0)]);

    let result = buffer
        .delete_backward_at_selections(selections.clone())
        .unwrap();

    assert!(result.is_none());
    assert_eq!(buffer.text().as_ref(), "abc");
    assert_eq!(buffer.version(), BufferVersion::INITIAL);
    assert_eq!(buffer.selection(), &selections);
}

#[test]
fn delete_forward_at_document_end_is_noop() {
    let mut buffer = buffer("abc");
    let selections = SelectionSet::new(vec![caret(3)]);

    let result = buffer
        .delete_forward_at_selections(selections.clone())
        .unwrap();

    assert!(result.is_none());
    assert_eq!(buffer.text().as_ref(), "abc");
    assert_eq!(buffer.version(), BufferVersion::INITIAL);
    assert_eq!(buffer.selection(), &selections);
}

#[test]
fn delete_backward_handles_multiple_carets_as_one_transaction() {
    let mut buffer = buffer("abcdef");
    let selections = SelectionSet::new(vec![caret(2), caret(5)]);

    buffer.delete_backward_at_selections(selections).unwrap();

    assert_eq!(buffer.text().as_ref(), "acdf");
    assert_eq!(buffer.history_status().undo_depth, 1);
    assert_eq!(buffer.selection().ranges(), vec![range(1, 1), range(3, 3)]);
}

#[test]
fn delete_forward_handles_multiple_carets_as_one_transaction() {
    let mut buffer = buffer("abcdef");
    let selections = SelectionSet::new(vec![caret(1), caret(4)]);

    buffer.delete_forward_at_selections(selections).unwrap();

    assert_eq!(buffer.text().as_ref(), "acdf");
    assert_eq!(buffer.history_status().undo_depth, 1);
    assert_eq!(buffer.selection().ranges(), vec![range(1, 1), range(3, 3)]);
}

#[test]
fn ordinary_transaction_maps_existing_selection_when_after_selection_is_not_explicit() {
    let mut buffer = buffer("abcd");
    buffer
        .set_selection(SelectionSet::new(vec![selection(2, 4)]))
        .unwrap();

    buffer.insert(CharOffset::new(1), "X").unwrap();

    assert_eq!(buffer.text().as_ref(), "aXbcd");
    assert_eq!(buffer.selection().ranges(), vec![range(3, 5)]);
}

#[test]
fn ordinary_transaction_maps_reversed_selection_without_losing_direction() {
    let mut buffer = buffer("abcd");
    buffer
        .set_selection(SelectionSet::new(vec![selection(4, 2)]))
        .unwrap();

    buffer.insert(CharOffset::new(1), "X").unwrap();

    let selection = buffer.selection().primary();
    assert_eq!(selection.anchor(), CharOffset::new(5));
    assert_eq!(selection.head(), CharOffset::new(3));
    assert!(selection.is_reversed());
    assert_eq!(selection.range(), range(3, 5));
}

#[test]
fn ordinary_transaction_collapses_selection_endpoint_inside_deleted_range() {
    let mut buffer = buffer("abcdef");
    buffer
        .set_selection(SelectionSet::new(vec![selection(2, 5)]))
        .unwrap();

    buffer.delete(range(1, 4)).unwrap();

    assert_eq!(buffer.text().as_ref(), "aef");
    assert_eq!(buffer.selection().ranges(), vec![range(1, 2)]);
}

#[test]
fn explicit_transaction_selection_overrides_automatic_mapping() {
    let mut buffer = buffer("hello");
    let before = SelectionSet::caret(CharOffset::new(1));
    let after = SelectionSet::caret(CharOffset::new(4));

    buffer.set_selection(before.clone()).unwrap();

    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(CharOffset::new(5), "!".to_string()).unwrap()],
    )
    .unwrap()
    .with_selection(Some(before.clone()), Some(after.clone()));

    buffer.apply_transaction(tx).unwrap();

    assert_eq!(buffer.text().as_ref(), "hello!");
    assert_eq!(buffer.selection(), &after);
}

#[test]
fn transaction_without_history_still_updates_selection() {
    let mut buffer = buffer("abc");
    let before = SelectionSet::caret(CharOffset::new(1));
    let after = SelectionSet::caret(CharOffset::new(4));

    buffer.set_selection(before.clone()).unwrap();

    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(CharOffset::new(3), "!".to_string()).unwrap()],
    )
    .unwrap()
    .with_metadata(TransactionMetadata::new(TransactionSource::Command).without_history())
    .with_selection(Some(before), Some(after.clone()));

    buffer.apply_transaction(tx).unwrap();

    assert_eq!(buffer.text().as_ref(), "abc!");
    assert_eq!(buffer.history_status().undo_depth, 0);
    assert_eq!(buffer.selection(), &after);
}

#[test]
fn undo_and_redo_restore_multi_cursor_selection() {
    let mut buffer = buffer("abcd");
    let before = SelectionSet::new(vec![caret(1), caret(3)]);

    buffer.insert_at_selections(before.clone(), "X").unwrap();
    assert_eq!(buffer.text().as_ref(), "aXbcXd");
    assert_eq!(buffer.selection().ranges(), vec![range(2, 2), range(5, 5)]);

    buffer.undo().unwrap();
    assert_eq!(buffer.text().as_ref(), "abcd");
    assert_eq!(buffer.selection(), &before);

    buffer.redo().unwrap();
    assert_eq!(buffer.text().as_ref(), "aXbcXd");
    assert_eq!(buffer.selection().ranges(), vec![range(2, 2), range(5, 5)]);
}

#[test]
fn merged_history_restores_outer_selection_boundaries() {
    let mut buffer = buffer("");
    let before = SelectionSet::caret(CharOffset::new(0));
    let middle = SelectionSet::caret(CharOffset::new(1));
    let after = SelectionSet::caret(CharOffset::new(2));

    let first = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(CharOffset::new(0), "a".to_string()).unwrap()],
    )
    .unwrap()
    .with_selection(Some(before.clone()), Some(middle.clone()));
    buffer.apply_transaction(first).unwrap();

    let second = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(CharOffset::new(1), "b".to_string()).unwrap()],
    )
    .unwrap()
    .with_metadata(
        TransactionMetadata::new(TransactionSource::Keyboard)
            .with_merge_policy(TransactionMergePolicy::MergeWithPrevious),
    )
    .with_selection(Some(middle), Some(after.clone()));
    buffer.apply_transaction(second).unwrap();

    assert_eq!(buffer.text().as_ref(), "ab");
    assert_eq!(buffer.history_status().undo_depth, 1);
    assert_eq!(buffer.selection(), &after);

    buffer.undo().unwrap();
    assert_eq!(buffer.text().as_ref(), "");
    assert_eq!(buffer.selection(), &before);

    buffer.redo().unwrap();
    assert_eq!(buffer.text().as_ref(), "ab");
    assert_eq!(buffer.selection(), &after);
}
