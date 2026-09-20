use super::*;
use zcv_text::{Buffer, BufferConfig};

fn snapshot_of(text: &str) -> MultiBufferSnapshot {
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    MultiBufferSnapshot::from(buffer.snapshot())
}

fn line_anchor(snapshot: &MultiBufferSnapshot, line: usize, end: bool) -> MultiBufferAnchor {
    let start = snapshot
        .line_start_byte(Line::new(line))
        .expect("测试行应存在");
    let offset = if end {
        snapshot
            .line_start_byte(Line::new(line + 1))
            .unwrap_or_else(|_| snapshot.len_bytes())
    } else {
        start
    };
    snapshot.anchor_at(
        offset,
        if end {
            Affinity::After
        } else {
            Affinity::Before
        },
    )
}

#[test]
fn creases_query_only_the_requested_line_range() {
    let snapshot = snapshot_of("aa\nbb\ncc\ndd\n");
    let mut map = CreaseMap::new(&snapshot);
    map.insert(
        [
            Crease::simple(line_anchor(&snapshot, 1, false)..line_anchor(&snapshot, 1, true)),
            Crease::simple(line_anchor(&snapshot, 3, false)..line_anchor(&snapshot, 3, true)),
        ],
        &snapshot,
    );
    let creases = map.snapshot();
    assert_eq!(creases.creases().count(), 2);
    assert_eq!(
        creases
            .creases_in_range(Line::new(0)..Line::new(2), &snapshot)
            .count(),
        1
    );
    assert_eq!(
        creases
            .creases_in_range(Line::new(2)..Line::new(4), &snapshot)
            .count(),
        1
    );
    assert!(creases.crease_at_line(Line::new(1), &snapshot).is_some());
    assert!(creases.crease_at_line(Line::new(0), &snapshot).is_none());
}

#[test]
fn removing_creases_drops_them_from_queries() {
    let snapshot = snapshot_of("aa\nbb\ncc\n");
    let mut map = CreaseMap::new(&snapshot);
    let first = line_anchor(&snapshot, 0, false)..line_anchor(&snapshot, 1, true);
    let second = line_anchor(&snapshot, 1, false)..line_anchor(&snapshot, 2, true);
    let ids = map.insert([Crease::simple(first), Crease::simple(second)], &snapshot);
    assert_eq!(map.snapshot().creases().count(), 2);

    map.remove([ids[0]], &snapshot);
    assert_eq!(map.snapshot().creases().count(), 1);
    assert!(
        map.snapshot()
            .crease_at_line(Line::new(0), &snapshot)
            .is_none()
    );
    assert!(
        map.snapshot()
            .crease_at_line(Line::new(1), &snapshot)
            .is_some()
    );
}
