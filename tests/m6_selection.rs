//! M6 机器契约：聚合 SelectionSet / 多光标、文本移动和 IME composition 行为。
//!
//! 小阶段测试保留在本文件的子模块中，避免一个大阶段拆出多个 cargo test 入口。

mod m6a_selection_set {
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
        .with_metadata(TransactionMetadata::new(TransactionSource::Programmatic).without_history())
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
            TransactionMetadata::new(TransactionSource::Programmatic)
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
}

mod m6b_word_movement {
    //! M6B 机器契约：锁定 grapheme、word、identifier、subword 与 symbol 的文本移动语义。
    //!
    //! 本文件只验证引擎层边界查找和 selection 移动，不绑定宿主快捷键或命令层策略。

    use zom_engine::{
        Buffer, BufferConfig, CharOffset, MovementDirection, MovementUnit, Selection, SelectionSet,
    };

    fn buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    fn c(value: usize) -> CharOffset {
        CharOffset::new(value)
    }

    #[test]
    fn word_movement_uses_unicode_word_boundaries() {
        let buffer = buffer("hello 世界 👋");

        assert_eq!(buffer.next_word_boundary(c(0)).unwrap(), c(5));
        assert_eq!(buffer.next_word_boundary(c(5)).unwrap(), c(6));
        assert_eq!(buffer.next_word_boundary(c(6)).unwrap(), c(8));

        assert_eq!(buffer.previous_word_boundary(c(8)).unwrap(), c(6));
        assert_eq!(buffer.previous_word_boundary(c(6)).unwrap(), c(5));
        assert_eq!(buffer.previous_word_boundary(c(5)).unwrap(), c(0));
    }

    #[test]
    fn identifier_movement_keeps_snake_case_and_dollar_identifiers_together() {
        let buffer = buffer("let foo_bar = baz$qux;");

        assert_eq!(buffer.next_identifier_boundary(c(0)).unwrap(), c(3));
        assert_eq!(buffer.next_identifier_boundary(c(3)).unwrap(), c(4));
        assert_eq!(buffer.next_identifier_boundary(c(4)).unwrap(), c(11));
        assert_eq!(buffer.next_identifier_boundary(c(14)).unwrap(), c(21));

        assert_eq!(buffer.previous_identifier_boundary(c(21)).unwrap(), c(14));
        assert_eq!(buffer.previous_identifier_boundary(c(14)).unwrap(), c(11));
        assert_eq!(buffer.previous_identifier_boundary(c(11)).unwrap(), c(4));
    }

    #[test]
    fn subword_movement_splits_snake_camel_pascal_and_digits() {
        let buffer = buffer("foo_barBaz42 HTTPServer");

        assert_eq!(buffer.next_subword_boundary(c(0)).unwrap(), c(3));
        assert_eq!(buffer.next_subword_boundary(c(3)).unwrap(), c(4));
        assert_eq!(buffer.next_subword_boundary(c(4)).unwrap(), c(7));
        assert_eq!(buffer.next_subword_boundary(c(7)).unwrap(), c(10));
        assert_eq!(buffer.next_subword_boundary(c(10)).unwrap(), c(12));

        assert_eq!(buffer.next_subword_boundary(c(13)).unwrap(), c(17));
        assert_eq!(buffer.next_subword_boundary(c(17)).unwrap(), c(23));

        assert_eq!(buffer.previous_subword_boundary(c(23)).unwrap(), c(17));
        assert_eq!(buffer.previous_subword_boundary(c(17)).unwrap(), c(13));
        assert_eq!(buffer.previous_subword_boundary(c(10)).unwrap(), c(7));
        assert_eq!(buffer.previous_subword_boundary(c(7)).unwrap(), c(4));
    }

    #[test]
    fn symbol_movement_targets_operator_and_punctuation_runs() {
        let buffer = buffer("foo::bar += baz");

        assert_eq!(buffer.next_symbol_boundary(c(0)).unwrap(), c(3));
        assert_eq!(buffer.next_symbol_boundary(c(3)).unwrap(), c(5));
        assert_eq!(buffer.next_symbol_boundary(c(5)).unwrap(), c(9));
        assert_eq!(buffer.next_symbol_boundary(c(9)).unwrap(), c(11));

        assert_eq!(buffer.previous_symbol_boundary(c(11)).unwrap(), c(9));
        assert_eq!(buffer.previous_symbol_boundary(c(9)).unwrap(), c(5));
        assert_eq!(buffer.previous_symbol_boundary(c(5)).unwrap(), c(3));
    }

    #[test]
    fn generic_movement_boundary_dispatches_by_unit() {
        let buffer = buffer("foo_bar");

        assert_eq!(
            buffer
                .movement_boundary(c(0), MovementDirection::Next, MovementUnit::Identifier)
                .unwrap(),
            c(7)
        );
        assert_eq!(
            buffer
                .movement_boundary(c(0), MovementDirection::Next, MovementUnit::Subword)
                .unwrap(),
            c(3)
        );
    }

    #[test]
    fn grapheme_movement_dispatches_to_existing_grapheme_boundaries() {
        let buffer = buffer("a🇨🇳b");

        assert_eq!(
            buffer
                .movement_boundary(c(1), MovementDirection::Next, MovementUnit::Grapheme)
                .unwrap(),
            c(3)
        );
        assert_eq!(
            buffer
                .movement_boundary(c(3), MovementDirection::Previous, MovementUnit::Grapheme)
                .unwrap(),
            c(1)
        );
    }

    #[test]
    fn move_current_selection_without_extend_collapses_to_new_caret() {
        let mut buffer = buffer("hello world");
        buffer.set_selection(SelectionSet::caret(c(0))).unwrap();

        let moved = buffer
            .move_current_selection(MovementDirection::Next, MovementUnit::Word, false)
            .unwrap();

        assert_eq!(moved.as_slice(), &[Selection::caret(c(5))]);
        assert_eq!(buffer.selection().as_slice(), &[Selection::caret(c(5))]);
    }

    #[test]
    fn move_current_selection_with_extend_preserves_anchor() {
        let mut buffer = buffer("hello world");
        buffer.set_selection(SelectionSet::caret(c(0))).unwrap();

        let moved = buffer
            .move_current_selection(MovementDirection::Next, MovementUnit::Word, true)
            .unwrap();

        assert_eq!(moved.as_slice(), &[Selection::new(c(0), c(5))]);
        assert_eq!(buffer.selection().as_slice(), &[Selection::new(c(0), c(5))]);
    }

    #[test]
    fn move_selections_supports_multi_cursor_heads() {
        let mut buffer = buffer("one two three four");
        let selections = SelectionSet::new(vec![Selection::caret(c(0)), Selection::caret(c(8))]);

        let moved = buffer
            .move_selections(
                selections,
                MovementDirection::Next,
                MovementUnit::Word,
                false,
            )
            .unwrap();

        assert_eq!(
            moved.as_slice(),
            &[Selection::caret(c(3)), Selection::caret(c(13))]
        );
    }

    #[test]
    fn movement_rejects_out_of_bounds_offsets() {
        let buffer = buffer("abc");

        assert!(
            buffer
                .next_word_boundary(CharOffset::new(4))
                .unwrap_err()
                .to_string()
                .contains("越界")
        );
    }
}

mod m6c_composition {
    //! M6C 机器契约：锁定 IME composition 的 start/update/commit/cancel、preedit 和 selection 恢复语义。
    //!
    //! 本文件只验证引擎状态机对外行为，不测试真实系统 IME 事件或 GPUI 输入法桥接。

    use zom_engine::{
        Buffer, BufferConfig, CharOffset, CompositionSelection, Selection, SelectionSet,
    };

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
}
