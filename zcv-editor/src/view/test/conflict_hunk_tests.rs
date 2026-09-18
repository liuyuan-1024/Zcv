use super::*;

fn multi_buffer_range(start: usize, end: usize) -> MultiBufferRange {
    MultiBufferRange::from(TextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap())
}

#[test]
fn conflict_hunk_maps_ours_and_theirs_ranges() {
    let hunk = EditorHunk::conflict("path\n0", 10..26, 20, |offset| offset).unwrap();
    assert_eq!(hunk.id, SharedString::from("path\n0"));
    assert_eq!(hunk.range, multi_buffer_range(10, 26));
    assert_eq!(hunk.parts.len(), 2);
    assert_eq!(hunk.parts[0].range, multi_buffer_range(10, 20));
    assert_eq!(hunk.parts[0].content_kind, DiffHunkKind::Deleted);
    assert_eq!(hunk.parts[0].marker_kind, EditorHunkMarkerKind::Conflict);
    assert_eq!(hunk.parts[1].range, multi_buffer_range(20, 26));
    assert_eq!(hunk.parts[1].content_kind, DiffHunkKind::Added);
    assert_eq!(hunk.parts[1].marker_kind, EditorHunkMarkerKind::Conflict);
}

#[test]
fn conflict_hunk_applies_coordinate_mapping() {
    let hunk = EditorHunk::conflict("path\n1", 4..12, 8, |offset| offset + 100).unwrap();
    assert_eq!(hunk.range, multi_buffer_range(104, 112));
    assert_eq!(hunk.parts[0].range, multi_buffer_range(104, 108));
    assert_eq!(hunk.parts[1].range, multi_buffer_range(108, 112));
}

#[test]
fn conflict_hunk_rejects_invalid_ranges() {
    let reversed = std::ops::Range { start: 8, end: 4 };
    assert!(EditorHunk::conflict("path\n2", reversed, 6, |offset| offset).is_none());
    assert!(EditorHunk::conflict("path\n3", 4..8, 12, |offset| offset).is_none());
}
