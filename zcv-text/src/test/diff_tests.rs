use super::*;
use crate::ByteOffset;
use crate::PositionMap;

fn map_offset(edits: &[Edit], offset: usize) -> usize {
    let position_map = PositionMap::from_edits(edits);
    position_map
        .map_old_position(ByteOffset::new(offset))
        .value()
        .get()
}

#[test]
fn identical_texts_produce_empty_edits() {
    assert!(diff_edits("a\nb\nc", "a\nb\nc").is_empty());
    assert!(diff_edits("", "").is_empty());
}

#[test]
fn line_insertion_maps_surrounding_offsets() {
    let edits = diff_edits("a\nb\nc", "a\nx\nb\nc");
    // "a\n" 匹配；光标在 "b" 行内 offset 2 处应平移到插入行之后。
    assert_eq!(map_offset(&edits, 2), 4);
    assert_eq!(map_offset(&edits, 6), 8);
}

#[test]
fn line_deletion_maps_offsets_to_delete_start() {
    let edits = diff_edits("a\nx\nb\nc", "a\nb\nc");
    // 被删行 "x\n"（offset 2..4）内的坐标塌缩到删除起点。
    assert_eq!(map_offset(&edits, 3), 2);
    assert_eq!(map_offset(&edits, 4), 2);
    assert_eq!(map_offset(&edits, 6), 4);
}

#[test]
fn inline_edit_replaces_whole_words() {
    let edits = diff_edits("alpha\nbravo\ncharlie", "alpha\nbrxavo\ncharlie");
    // 词级 diff 以单词为单位：整个 "bravo" 被 "brxavo" 替换。
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].range().start().get(), 6);
    assert_eq!(edits[0].range().end().get(), 11);
    assert_eq!(edits[0].replacement(), "brxavo");
    // 词内坐标塌缩到替换段起点。
    assert_eq!(map_offset(&edits, 8), 6);
}

#[test]
fn complete_rewrite_refines_to_words() {
    let edits = diff_edits("alpha\nbravo", "xyz\nqwerty");
    // 两个单词各自替换，换行保持不变。
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0].range().start().get(), 0);
    assert_eq!(edits[0].range().end().get(), 5);
    assert_eq!(edits[0].replacement(), "xyz");
    assert_eq!(edits[1].range().start().get(), 6);
    assert_eq!(edits[1].range().end().get(), 11);
    assert_eq!(edits[1].replacement(), "qwerty");
    // 被替换内容内的坐标塌缩到替换段起点。
    assert_eq!(map_offset(&edits, 2), 0);
}

#[test]
fn empty_line_changes_map_correctly() {
    let edits = diff_edits("a\n\nb", "a\n\n\nb");
    // 中间插入一个空行：第二个空行前的 "a\n\n" 匹配，"b" 后移一行。
    assert_eq!(map_offset(&edits, 4), 5);
}
