use super::*;
use crate::{ByteOffset, Edit, TextRange};

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn patch(old: TextRange, new_len: usize) -> TextPatch {
    let edit = if old.is_empty() {
        Edit::insert(old.start(), "x".repeat(new_len)).unwrap()
    } else {
        Edit::replace(old, "y".repeat(new_len))
    };
    TextPatch::from_edit_list(std::slice::from_ref(&edit))
}

#[test]
fn coordinate_index_composes_across_chunk_boundaries() {
    let mut index = CoordinateIndex::default();
    let total = CHUNK * 2 + 3;
    let mut version = BufferVersion::INITIAL;
    for _ in 0..total {
        let next = version.next().unwrap();
        index = index.appended(version, next, patch(TextRange::new(b(0), b(0)).unwrap(), 1));
        version = next;
    }

    let composed = index
        .patch_since(BufferVersion::INITIAL, version)
        .expect("不衰减索引必须覆盖全部版本");
    assert_eq!(composed.edits().len(), 1);
    assert_eq!(composed.edits()[0].new_range().len(), total);
}

#[test]
fn coordinate_index_maps_a_middle_version() {
    let mut index = CoordinateIndex::default();
    let v0 = BufferVersion::INITIAL;
    let v1 = v0.next().unwrap();
    let v2 = v1.next().unwrap();
    index = index.appended(v0, v1, patch(TextRange::new(b(0), b(1)).unwrap(), 3));
    index = index.appended(v1, v2, patch(TextRange::new(b(2), b(2)).unwrap(), 1));

    let from_v0 = index.patch_since(v0, v2).unwrap();
    let from_v1 = index.patch_since(v1, v2).unwrap();
    assert_ne!(from_v0, from_v1, "不同起点的组合结果必须不同");
    assert_eq!(from_v1.edits().len(), 1);
    assert_eq!(
        from_v1.edits()[0].old_range(),
        TextRange::new(b(2), b(2)).unwrap()
    );
}
