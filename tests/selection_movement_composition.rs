use zom_engine::*;

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

fn selection(anchor: usize, head: usize) -> Selection {
    Selection::new(b(anchor), b(head))
}

fn caret(offset: usize) -> Selection {
    Selection::caret(b(offset))
}

fn set_caret(offset: usize) -> SelectionSet {
    SelectionSet::caret(b(offset))
}

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

#[test]
fn selection_and_cursor_contract_should_preserve_anchor_head_direction_and_range() {
    let cursor = Cursor::new(b(3));
    let reversed = selection(7, 2);

    assert_eq!(cursor.offset(), b(3));
    assert_eq!(cursor.to_selection(), caret(3));
    assert_eq!(reversed.anchor(), b(7));
    assert_eq!(reversed.head(), b(2));
    assert!(reversed.is_reversed());
    assert_eq!(reversed.range(), range(2, 7));
    assert_eq!(reversed.collapse_to_start(), caret(2));
    assert_eq!(reversed.collapse_to_end(), caret(7));
}

#[test]
fn selection_set_normalization_should_sort_merge_duplicates_and_preserve_primary() {
    let set = SelectionSet::new_with_primary(
        vec![caret(8), selection(4, 2), caret(1), selection(3, 6)],
        1,
    );

    assert_eq!(set.ranges(), vec![range(1, 1), range(2, 6), range(8, 8)]);
    assert_eq!(set.primary_index(), 1);
    assert_eq!(set.primary().range(), range(2, 6));

    let adjacent = SelectionSet::new_with_policy(
        vec![selection(1, 3), selection(3, 5)],
        0,
        SelectionMergePolicy::MergeOverlappingOrAdjacent,
    );
    assert_eq!(adjacent.ranges(), vec![range(1, 5)]);
}

#[test]
fn set_selection_should_reject_out_of_bounds_invalid_utf8_and_grapheme_middle_offsets_atomically() {
    for (text, offset) in [("abc", 4), ("你a", 1), ("ae\u{301}b", 2), ("a\r\nb", 2)] {
        let mut buffer = buffer(text);
        let original = buffer.selection().clone();

        assert!(buffer.set_selection(set_caret(offset)).is_err());
        assert_eq!(buffer.selection(), &original);
    }
}

#[test]
fn multi_cursor_insert_replace_delete_should_apply_one_state_transition_per_command() {
    let mut buffer = buffer("abcdef");

    buffer
        .insert_at_selections(SelectionSet::new(vec![caret(1), caret(4)]), "X")
        .unwrap();
    assert_eq!(buffer.text().as_ref(), "aXbcdXef");
    assert_eq!(buffer.history_status().undo_depth, 1);
    assert_eq!(buffer.selection().ranges(), vec![range(2, 2), range(6, 6)]);

    buffer
        .replace_selections(
            SelectionSet::new(vec![selection(1, 3), selection(5, 7)]),
            "Q",
        )
        .unwrap();
    assert_eq!(buffer.text().as_ref(), "aQcdQf");
    assert_eq!(buffer.selection().ranges(), vec![range(2, 2), range(5, 5)]);

    buffer
        .delete_selection_ranges(SelectionSet::new(vec![selection(1, 2), selection(4, 5)]))
        .unwrap();
    assert_eq!(buffer.text().as_ref(), "acdf");
    assert_eq!(buffer.selection().ranges(), vec![range(1, 1), range(3, 3)]);
}

#[test]
fn delete_backward_and_forward_at_selections_should_respect_grapheme_clusters() {
    let mut backward = buffer("ae\u{301}b");
    backward
        .delete_backward_at_selections(set_caret(4))
        .unwrap();
    assert_eq!(backward.text().as_ref(), "ab");
    assert_eq!(backward.selection().ranges(), vec![range(1, 1)]);

    let mut forward = buffer("ae\u{301}b");
    forward.delete_forward_at_selections(set_caret(1)).unwrap();
    assert_eq!(forward.text().as_ref(), "ab");
    assert_eq!(forward.selection().ranges(), vec![range(1, 1)]);
}

#[test]
fn ordinary_transaction_without_explicit_selection_should_map_existing_selection() {
    let mut buffer = buffer("abcd");
    buffer
        .set_selection(SelectionSet::new(vec![selection(4, 2)]))
        .unwrap();

    buffer.insert(b(1), "X").unwrap();

    let mapped = buffer.selection().primary();
    assert_eq!(mapped.anchor(), b(5));
    assert_eq!(mapped.head(), b(3));
    assert_eq!(mapped.range(), range(3, 5));
}

#[test]
fn movement_boundaries_should_dispatch_by_unit_and_reject_invalid_offsets() {
    let buffer = buffer("foo_barBaz42 += 世界");

    assert_eq!(buffer.next_word_boundary(c(0)).unwrap(), c(3));
    assert_eq!(buffer.next_identifier_boundary(c(0)).unwrap(), c(12));
    assert_eq!(buffer.next_subword_boundary(c(0)).unwrap(), c(3));
    assert_eq!(buffer.next_symbol_boundary(c(13)).unwrap(), c(15));
    assert_eq!(
        buffer
            .movement_boundary(c(0), MovementDirection::Next, MovementUnit::Subword)
            .unwrap(),
        c(3)
    );
    assert!(buffer.next_word_boundary(c(99)).is_err());
}

#[test]
fn move_current_selection_should_collapse_or_extend_from_existing_anchor() {
    let mut buffer = buffer("hello world");

    buffer.set_selection(set_caret(0)).unwrap();
    assert_eq!(
        buffer
            .move_current_selection(MovementDirection::Next, MovementUnit::Word, false)
            .unwrap()
            .as_slice(),
        &[caret(5)]
    );

    buffer.set_selection(set_caret(0)).unwrap();
    assert_eq!(
        buffer
            .move_current_selection(MovementDirection::Next, MovementUnit::Word, true)
            .unwrap()
            .as_slice(),
        &[selection(0, 5)]
    );
}

#[test]
fn composition_update_commit_cancel_should_share_transaction_pipeline_and_history_contract() {
    let mut buffer = buffer("hello");
    buffer.set_selection(set_caret(5)).unwrap();

    buffer.start_composition().unwrap();
    buffer.update_composition("世", None).unwrap();
    assert_eq!(buffer.text().as_ref(), "hello世");
    assert_eq!(buffer.composition().unwrap().range(), range(5, 8));
    assert!(!buffer.can_undo());

    buffer.update_composition("世界", None).unwrap();
    buffer.commit_composition("世界").unwrap();
    assert_eq!(buffer.text().as_ref(), "hello世界");
    assert_eq!(buffer.selection().primary().head(), b(11));
    assert!(buffer.can_undo());

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer.text().as_ref(), "hello");
    assert_eq!(buffer.selection().primary().head(), b(5));
}

#[test]
fn composition_cancel_should_restore_original_text_selection_and_dirty_flag() {
    let mut buffer = buffer("abc");
    buffer.mark_saved();
    buffer.set_selection(set_caret(1)).unwrap();

    buffer.start_composition().unwrap();
    buffer.update_composition("中", None).unwrap();
    let result = buffer.cancel_composition().unwrap();

    assert!(result.is_some());
    assert_eq!(buffer.text().as_ref(), "abc");
    assert_eq!(buffer.selection().primary().head(), b(1));
    assert!(!buffer.is_dirty());
    assert!(!buffer.is_composing());
}

#[test]
fn composition_relative_selection_inside_grapheme_cluster_should_be_rejected_atomically() {
    let mut buffer = buffer("ab");
    buffer.set_selection(set_caret(1)).unwrap();
    buffer.start_composition().unwrap();

    let err = buffer
        .update_composition("e\u{0301}", Some(CompositionSelection::caret(b(1))))
        .unwrap_err();

    assert!(matches!(
        err,
        EngineError::Coordinate(CoordinateError::InvalidGraphemeBoundary(_))
    ));
    assert!(buffer.is_composing());
    assert_eq!(buffer.text().as_ref(), "ab");
}
