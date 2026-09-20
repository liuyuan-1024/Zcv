use super::*;

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

#[test]
fn position_map_should_expose_affinity_stickiness_and_point_mapping() {
    let map = PositionMap::from_edits(&[Edit::replace(range(1, 3), "XYZ".to_string())]);

    assert!(matches!(map.map_old_position(b(2)), MappingResult::Deleted(pos) if pos == b(1)));
    assert_eq!(
        map.map_old_range_with_stickiness(range(1, 3), Stickiness::Expand)
            .value(),
        range(1, 4)
    );
    assert_eq!(map.map_old_position(b(0)).value(), b(0));
    assert_eq!(map.map_old_position(b(4)).value(), b(5));
}

#[test]
fn position_map_should_not_panic_when_mapped_range_endpoints_cross() {
    let map = PositionMap::from_edits(&[Edit::insert(b(0), "X".to_string()).unwrap()]);

    assert_eq!(
        map.map_old_range_with_stickiness(range(0, 0), Stickiness::Never),
        MappingResult::Collapsed(range(0, 0))
    );
}
