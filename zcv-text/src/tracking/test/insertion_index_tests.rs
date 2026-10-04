use super::*;
use crate::transaction::{Edit, EditList};
use crate::types::{ByteOffset, TextRange};

fn edit_list(edits: Vec<Edit>) -> EditList {
    EditList::new(edits).expect("测试编辑必须合法")
}

fn replace(range: std::ops::Range<usize>, text: &str) -> Edit {
    Edit::new(
        TextRange::new(ByteOffset::new(range.start), ByteOffset::new(range.end)).unwrap(),
        text,
    )
}

fn position(index: &InsertionIndex, offset: usize) -> InsertionPosition {
    index.position_at(offset).expect("位置必须存在")
}

fn apply(mut index: InsertionIndex, edits: Vec<Edit>, version: u64) -> InsertionIndex {
    index = index.with_edits(&edit_list(edits), BufferVersion::new(version));
    index
}

#[test]
fn sequential_insertions_keep_stable_order() {
    let mut index = InsertionIndex::new();
    index = apply(index, vec![replace(0..0, "abc")], 1);
    index = apply(index, vec![replace(3..3, "def")], 2);
    index = apply(index, vec![replace(0..0, "xy")], 3);
    assert!(index.position_at(7).is_some());
    assert!(index.position_at(8).is_some());
    assert!(index.position_at(9).is_none());

    let first = position(&index, 0);
    let second = position(&index, 2);
    let third = position(&index, 5);
    assert!(index.locator_of(first.id, first.offset) < index.locator_of(second.id, second.offset));
    assert!(index.locator_of(second.id, second.offset) < index.locator_of(third.id, third.offset));
}

#[test]
fn insertion_inside_a_run_sorts_between_split_halves() {
    let mut index = InsertionIndex::new();
    index = apply(index, vec![replace(0..0, "abcdef")], 1);
    index = apply(index, vec![replace(3..3, "XY")], 2);
    assert!(index.position_at(7).is_some());
    assert!(index.position_at(8).is_some());
    assert!(index.position_at(9).is_none());

    let before = position(&index, 0);
    let inserted = position(&index, 3);
    let after = position(&index, 5);
    assert!(
        index.locator_of(before.id, before.offset) < index.locator_of(inserted.id, inserted.offset)
    );
    assert!(
        index.locator_of(inserted.id, inserted.offset) < index.locator_of(after.id, after.offset)
    );
}

#[test]
fn deletion_keeps_identity_of_surviving_text() {
    let mut index = InsertionIndex::new();
    index = apply(index, vec![replace(0..0, "abcdef")], 1);
    let before = position(&index, 0);
    let surviving = position(&index, 5);
    index = apply(index, vec![replace(1..3, "")], 2);
    assert!(index.position_at(3).is_some());
    assert!(index.position_at(4).is_some());
    assert!(index.position_at(5).is_none());
    let after = position(&index, 3);
    assert_eq!(surviving.id, after.id);
    assert!(index.locator_of(before.id, before.offset) < index.locator_of(after.id, after.offset));
}

#[test]
fn visibility_changes_match_fragment_semantics() {
    let base = InsertionIndex::with_text("hello".len());
    assert!(!base.has_edits_since(BufferVersion::INITIAL));

    // 插入新片段：相对插入前是编辑，相对插入后不是。
    let inserted = apply(base.clone(), vec![replace(5..5, "!")], 1);
    assert!(inserted.has_edits_since(BufferVersion::INITIAL));
    assert!(!inserted.has_edits_since(BufferVersion::new(1)));

    // 插入后删除：片段在 INITIAL 时不可见、现在也不可见，判为无编辑；相对 v1 则判为有编辑。
    let removed = apply(inserted, vec![replace(5..6, "")], 2);
    assert!(!removed.has_edits_since(BufferVersion::INITIAL));
    assert!(removed.has_edits_since(BufferVersion::new(1)));

    // 删除既有文本：片段从可见变为不可见，判为有编辑。
    let deleted = apply(base, vec![replace(0..1, "")], 3);
    assert!(deleted.has_edits_since(BufferVersion::INITIAL));
}

#[test]
fn shared_index_keeps_older_versions_immutable_after_splits_and_undo() {
    let original = InsertionIndex::with_text(6);
    let original_position = position(&original, 2);
    let original_locator = original
        .locator_of(original_position.id, original_position.offset)
        .cloned();

    let inserted = apply(original.clone(), vec![replace(3..3, "X")], 1);
    let deleted = apply(inserted.clone(), vec![replace(1..2, "")], 2);
    let restored = deleted.undone(
        BufferVersion::new(1),
        BufferVersion::new(2),
        BufferVersion::new(3),
    );

    assert!(original.position_at(6).is_some());
    assert!(original.position_at(7).is_none());
    assert_eq!(
        original.locator_of(original_position.id, original_position.offset),
        original_locator.as_ref()
    );
    assert_ne!(
        inserted.locator_of(original_position.id, original_position.offset),
        original_locator.as_ref()
    );
    assert!(inserted.position_at(7).is_some());
    assert!(deleted.position_at(7).is_none());
    assert!(restored.position_at(7).is_some());
    assert!(!restored.has_edits_since(BufferVersion::new(1)));
}

#[test]
fn repeated_splits_keep_every_offset_of_the_original_insertion_addressable() {
    let mut index = InsertionIndex::with_text(8);
    let id = position(&index, 0).id;
    index = apply(index, vec![replace(2..2, "X")], 1);
    index = apply(index, vec![replace(5..5, "Y")], 2);
    index = apply(index, vec![replace(7..8, "")], 3);

    for offset in 0..=8 {
        assert!(index.locator_of(id, offset).is_some());
    }
    assert!(index.position_at(9).is_some());
    assert!(index.position_at(10).is_none());
    assert!(index.locator_of(InsertionId(99), 0).is_none());
}
