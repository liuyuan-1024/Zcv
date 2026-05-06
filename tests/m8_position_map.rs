//! M8 机器契约：锁定 PositionMap 的强类型 old/new 坐标映射与 ChangeSet 互操作。
//!
//! 本文件只验证 M8A PositionMap，不测试 DeltaEvent 队列、anchor stickiness 或 UI testbed。

use zom_engine::{
    Affinity, Bias, Buffer, BufferConfig, CharOffset, Edit, MappingResult, PositionMap, Stickiness,
    TextRange, Transaction,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(c(start), c(end)).unwrap()
}

fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> PositionMap {
    let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
    let (_, changeset) = buffer.apply_transaction(tx).unwrap();
    changeset.position_map()
}

#[test]
fn position_map_maps_old_positions_to_new_positions() {
    let mut buffer = buffer("12345");
    let map = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);

    assert_eq!(buffer.text(), "125");
    assert_eq!(map.map_old_position(c(0)), MappingResult::Mapped(c(0)));
    assert_eq!(map.map_old_position(c(2)), MappingResult::Deleted(c(2)));
    assert_eq!(map.map_old_position(c(3)), MappingResult::Deleted(c(2)));
    assert_eq!(map.map_old_position(c(5)), MappingResult::Mapped(c(3)));
}

#[test]
fn position_map_maps_new_positions_back_to_old_positions_with_ambiguity() {
    let mut buffer = buffer("ab");
    let map = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    assert_eq!(buffer.text(), "aXYZb");
    assert_eq!(map.map_old_position(c(1)), MappingResult::Mapped(c(4)));
    assert_eq!(map.map_new_position(c(0)), MappingResult::Mapped(c(0)));
    assert_eq!(map.map_new_position(c(2)), MappingResult::Ambiguous(c(1)));
    assert_eq!(map.map_new_position(c(4)), MappingResult::Mapped(c(1)));
    assert_eq!(map.map_new_position(c(5)), MappingResult::Mapped(c(2)));
}

#[test]
fn affinity_controls_old_position_at_insert_boundary() {
    let mut buffer = buffer("ab");
    let map = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    assert_eq!(
        map.map_old_position_with_affinity(c(1), Affinity::Before),
        MappingResult::Mapped(c(1))
    );
    assert_eq!(
        map.map_old_position_with_affinity(c(1), Affinity::After),
        MappingResult::Mapped(c(4))
    );
}

#[test]
fn bias_controls_new_position_inside_replacement_ambiguity() {
    let mut buffer = buffer("abcdef");
    let map = apply(
        &mut buffer,
        vec![Edit::replace(range(1, 3), "XYZ".to_string())],
    );

    assert_eq!(
        map.map_new_position_with_bias(c(2), Bias::Left),
        MappingResult::Ambiguous(c(1))
    );
    assert_eq!(
        map.map_new_position_with_bias(c(2), Bias::Right),
        MappingResult::Ambiguous(c(3))
    );
}

#[test]
fn old_range_fully_deleted_maps_to_collapsed_range() {
    let mut buffer = buffer("12345");
    let map = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);

    assert_eq!(
        map.map_old_range(range(2, 4)),
        MappingResult::Collapsed(range(2, 2))
    );
}

#[test]
fn new_ranges_touching_deleted_boundary_do_not_all_become_ambiguous() {
    let mut buffer = buffer("12345");
    let map = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);

    assert_eq!(
        map.map_new_range(range(0, 2)),
        MappingResult::Mapped(range(0, 2))
    );
    assert_eq!(
        map.map_new_range(range(2, 3)),
        MappingResult::Mapped(range(4, 5))
    );
    assert_eq!(
        map.map_new_range(range(2, 2)),
        MappingResult::Ambiguous(range(2, 2))
    );
}

#[test]
fn stickiness_controls_range_growth_at_insert_boundaries() {
    let mut buffer = buffer("abcd");
    let map = apply(
        &mut buffer,
        vec![
            Edit::insert(c(1), "X".to_string()).unwrap(),
            Edit::insert(c(3), "Y".to_string()).unwrap(),
        ],
    );

    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 3), Stickiness::Never),
        MappingResult::Mapped(range(2, 4))
    );
    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 3), Stickiness::Expand),
        MappingResult::Mapped(range(1, 5))
    );
    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 3), Stickiness::BeforeInsertion),
        MappingResult::Mapped(range(1, 4))
    );
    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 3), Stickiness::AfterInsertion),
        MappingResult::Mapped(range(2, 5))
    );
}

#[test]
fn stickiness_can_expand_empty_range_to_inserted_text() {
    let mut buffer = buffer("ab");
    let map = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 1), Stickiness::Expand),
        MappingResult::Mapped(range(1, 4))
    );
    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 1), Stickiness::BeforeInsertion),
        MappingResult::Mapped(range(1, 1))
    );
    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 1), Stickiness::AfterInsertion),
        MappingResult::Mapped(range(4, 4))
    );
}

#[test]
fn replacement_reports_deleted_old_range_and_ambiguous_new_range() {
    let mut buffer = buffer("abcdef");
    let map = apply(
        &mut buffer,
        vec![Edit::replace(range(1, 3), "XYZ".to_string())],
    );

    assert_eq!(buffer.text(), "aXYZdef");
    assert_eq!(
        map.map_old_range(range(1, 3)),
        MappingResult::Deleted(range(1, 4))
    );
    assert_eq!(
        map.map_new_range(range(1, 4)),
        MappingResult::Ambiguous(range(1, 3))
    );
}

#[test]
fn old_range_expands_across_insertions_without_becoming_deleted() {
    let mut buffer = buffer("ab");
    let map = apply(
        &mut buffer,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    assert_eq!(
        map.map_old_range(range(0, 2)),
        MappingResult::Mapped(range(0, 5))
    );
}

#[test]
fn changeset_and_position_map_interoperate_without_changing_legacy_mapping() {
    let mut buffer = buffer("abcdef");
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::replace(range(1, 3), "XYZ".to_string())],
    )
    .unwrap();

    let (_, changeset) = buffer.apply_transaction(tx).unwrap();
    let map = PositionMap::from_change_set(&changeset);

    assert_eq!(map.len(), 1);
    assert_eq!(map.map_old_position(c(1)), MappingResult::Deleted(c(1)));
    assert_eq!(map.map_old_position(c(3)), MappingResult::Mapped(c(4)));
}
