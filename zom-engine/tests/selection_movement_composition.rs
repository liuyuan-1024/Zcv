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

// ====================================================================
// Tab / Shift-Tab：标准 IDE 语义
// ====================================================================

fn buffer_with_tab(text: &str, indent_width: usize, insert_spaces: bool) -> Buffer {
    use std::num::NonZeroUsize;
    let mut config = BufferConfig::default();
    let nz = NonZeroUsize::new(indent_width).unwrap();
    config.tab = TabConfig::new(nz, nz, insert_spaces);
    Buffer::from_text(text.to_string(), config).unwrap()
}

#[test]
fn indent_at_caret_should_insert_soft_tab_aligned_to_next_indent_stop() {
    let mut buffer = buffer("abc");
    // caret 在 col 2 → 软 tab = 4 − 2%4 = 2 个空格，对齐到 col 4。
    buffer.indent_at_selections(set_caret(2)).unwrap();
    assert_eq!(buffer.text().as_ref(), "ab  c");
    assert_eq!(buffer.selection().primary().head(), b(4));
}

#[test]
fn indent_at_caret_on_first_column_should_insert_full_indent_width() {
    let mut buffer = buffer("abc");
    // caret 在 col 0 → 软 tab = 4 个空格。
    buffer.indent_at_selections(set_caret(0)).unwrap();
    assert_eq!(buffer.text().as_ref(), "    abc");
    assert_eq!(buffer.selection().primary().head(), b(4));
}

#[test]
fn indent_at_caret_after_existing_tab_should_account_for_display_column() {
    // 已有 "\t" 展开占 col 0..4，caret 在 col 4 → 软 tab = 4 个空格。
    let mut buffer = buffer("\tabc");
    buffer.indent_at_selections(set_caret(1)).unwrap();
    assert_eq!(buffer.text().as_ref(), "\t    abc");
    assert_eq!(buffer.selection().primary().head(), b(5));
}

#[test]
fn indent_with_multiple_carets_should_apply_per_caret_soft_tabs() {
    // 两个 caret 各自按所在 display column 计算软 tab。
    let mut buffer = buffer("abc\ndefg");
    let set = SelectionSet::new(vec![caret(1), caret(6)]);
    // caret 1 col 1 → 3 空格；caret 2 col 2 → 2 空格。
    buffer.indent_at_selections(set).unwrap();
    assert_eq!(buffer.text().as_ref(), "a   bc\nde  fg");
}

#[test]
fn indent_with_non_empty_selection_should_indent_lines_without_replacing_content() {
    // 单行非空选区：Tab 不该替换选中内容，而是按行块缩进。
    let mut buffer = buffer("abc");
    let set = SelectionSet::new(vec![selection(1, 2)]);
    buffer.indent_at_selections(set).unwrap();
    assert_eq!(buffer.text().as_ref(), "    abc");
}

#[test]
fn indent_with_multi_line_selection_should_indent_each_touched_line_once() {
    let mut buffer = buffer("ab\ncd\nef");
    // 跨 line 0..line 1 的选区：缩进 line 0 与 line 1，line 2 不动。
    let set = SelectionSet::new(vec![selection(0, 4)]);
    buffer.indent_at_selections(set).unwrap();
    assert_eq!(buffer.text().as_ref(), "    ab\n    cd\nef");
}

#[test]
fn indent_with_insert_spaces_false_should_insert_literal_tab_character() {
    let mut buffer = buffer_with_tab("ab", 4, false);
    buffer.indent_at_selections(set_caret(1)).unwrap();
    assert_eq!(buffer.text().as_ref(), "a\tb");
    assert_eq!(buffer.selection().primary().head(), b(2));
}

#[test]
fn outdent_should_remove_leading_indent_regardless_of_selection_shape() {
    // caret 行：删除前导 4 空格。
    let mut buf = buffer("    abc");
    buf.outdent_at_selections(set_caret(5)).unwrap();
    assert_eq!(buf.text().as_ref(), "abc");

    // 行首是真 tab：只删 tab。
    let mut buf = buffer("\tabc");
    buf.outdent_at_selections(set_caret(2)).unwrap();
    assert_eq!(buf.text().as_ref(), "abc");

    // 行首空白不足 indent_width：尽量删。
    let mut buf = buffer("  abc");
    buf.outdent_at_selections(set_caret(3)).unwrap();
    assert_eq!(buf.text().as_ref(), "abc");
}

// ====================================================================
// LineStep（上 / 下移动一行）
// ====================================================================

#[test]
fn line_step_should_move_caret_to_same_display_column_on_target_line() {
    let mut buffer = buffer("abcdef\nghijkl");
    // caret 在 line 0 col 3（"abc|def"）。
    let after = buffer
        .move_selections(
            set_caret(3),
            MovementDirection::Next,
            Motion::LineStep,
            false,
        )
        .unwrap();
    // 下移：落到 line 1 col 3（"ghi|jkl" = byte 7 + 3 = byte 10）。
    assert_eq!(after.primary().head(), b(10));

    // 再上移回 line 0 col 3。
    let after = buffer
        .move_selections(after, MovementDirection::Previous, Motion::LineStep, false)
        .unwrap();
    assert_eq!(after.primary().head(), b(3));
}

#[test]
fn line_step_should_clamp_to_line_end_on_shorter_target_line() {
    let mut buffer = buffer("abcdef\nxy");
    // caret 在 line 0 col 5（"abcde|f"）。
    let after = buffer
        .move_selections(
            set_caret(5),
            MovementDirection::Next,
            Motion::LineStep,
            false,
        )
        .unwrap();
    // line 1 只有 2 列，应当 clamp 到行尾（byte 7 + 2 = byte 9）。
    assert_eq!(after.primary().head(), b(9));
}

#[test]
fn line_step_at_first_line_previous_should_land_at_document_start() {
    let mut buffer = buffer("abc\ndef");
    let after = buffer
        .move_selections(
            set_caret(2),
            MovementDirection::Previous,
            Motion::LineStep,
            false,
        )
        .unwrap();
    assert_eq!(after.primary().head(), b(0));
}

#[test]
fn line_step_at_last_line_next_should_land_at_document_end() {
    let mut buffer = buffer("abc\ndef");
    // caret 在 line 1 col 1。
    let after = buffer
        .move_selections(
            set_caret(5),
            MovementDirection::Next,
            Motion::LineStep,
            false,
        )
        .unwrap();
    // 末行再下：跳到文档末尾。
    assert_eq!(after.primary().head(), buffer.len_bytes());
}

#[test]
fn line_step_should_preserve_anchor_when_extending_selection() {
    let mut buffer = buffer("abcdef\nghijkl");
    // 选区 anchor=byte 1, head=byte 3（已选 "bc"）。
    let initial = SelectionSet::new(vec![selection(1, 3)]);
    let after = buffer
        .move_selections(initial, MovementDirection::Next, Motion::LineStep, true)
        .unwrap();
    // 扩展：anchor 保留 1，head 下移到 line 1 col 3 = byte 10。
    let primary = *after.primary();
    assert_eq!(primary.anchor(), b(1));
    assert_eq!(primary.head(), b(10));
}

// ====================================================================
// PageStep（按 N 行翻页）
// ====================================================================

#[test]
fn page_step_should_jump_n_lines_keeping_display_column() {
    // 5 行，每行 6 列。
    let mut buffer = buffer("aaaaaa\nbbbbbb\ncccccc\ndddddd\neeeeee");
    // caret 在 line 0 col 3，PageDown 跳 2 行：line 2 col 3。
    let after = buffer
        .move_selections(
            set_caret(3),
            MovementDirection::Next,
            Motion::PageStep { lines: 2 },
            false,
        )
        .unwrap();
    // line 2 起始 byte = 14；col 3 → byte 17。
    assert_eq!(after.primary().head(), b(17));

    // 反向跳 1 行：回到 line 1 col 3 = byte 10。
    let after = buffer
        .move_selections(
            after,
            MovementDirection::Previous,
            Motion::PageStep { lines: 1 },
            false,
        )
        .unwrap();
    assert_eq!(after.primary().head(), b(10));
}

#[test]
fn page_step_should_clamp_to_last_line_when_lines_exceeds_remaining() {
    let mut buffer = buffer("aa\nbb\ncc");
    // caret 在 line 0，PageDown 跳 10 行：超出末行 → 落在末行同列（line 2 col 1）。
    let after = buffer
        .move_selections(
            set_caret(1),
            MovementDirection::Next,
            Motion::PageStep { lines: 10 },
            false,
        )
        .unwrap();
    // line 2 起始 byte = 6；col 1 → byte 7。
    assert_eq!(after.primary().head(), b(7));
}

#[test]
fn page_step_at_last_line_next_should_land_at_document_end() {
    let mut buffer = buffer("aa\nbb\ncc");
    // caret 在 line 2，PageDown：已在末行 → 文档末尾。
    let after = buffer
        .move_selections(
            set_caret(6),
            MovementDirection::Next,
            Motion::PageStep { lines: 5 },
            false,
        )
        .unwrap();
    assert_eq!(after.primary().head(), buffer.len_bytes());
}

#[test]
fn page_step_at_first_line_previous_should_land_at_document_start() {
    let mut buffer = buffer("aa\nbb\ncc");
    // caret 在 line 0 col 1，PageUp：已在首行 → 文档开头。
    let after = buffer
        .move_selections(
            set_caret(1),
            MovementDirection::Previous,
            Motion::PageStep { lines: 5 },
            false,
        )
        .unwrap();
    assert_eq!(after.primary().head(), b(0));
}

#[test]
fn outdent_should_be_noop_when_no_leading_whitespace() {
    let mut buffer = buffer("abc");
    let result = buffer.outdent_at_selections(set_caret(1)).unwrap();
    // 无任何前导空白：不产生事务。
    assert!(result.is_none());
    assert_eq!(buffer.text().as_ref(), "abc");
}
