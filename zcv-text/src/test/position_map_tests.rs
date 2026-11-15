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

#[test]
fn position_map_should_map_new_positions_back_to_the_old_version() {
    let map = PositionMap::from_edits(&[Edit::replace(range(1, 3), "XYZ".to_string())]);

    for (new, old) in [(0, 0), (1, 1), (2, 2), (3, 3), (4, 3)] {
        assert_eq!(
            map.map_new_position_with_affinity(b(new), Affinity::After),
            MappingResult::Mapped(b(old))
        );
    }
}

#[test]
fn position_map_should_attach_new_insertion_positions_to_the_insertion_point() {
    let map = PositionMap::from_edits(&[Edit::insert(b(1), "XY".to_string()).unwrap()]);

    for (new, old) in [(0, 0), (3, 1), (4, 2)] {
        assert_eq!(
            map.map_new_position_with_affinity(b(new), Affinity::After),
            MappingResult::Mapped(b(old))
        );
    }
    for new in [1, 2] {
        assert_eq!(
            map.map_new_position_with_affinity(b(new), Affinity::After),
            MappingResult::Ambiguous(b(1))
        );
    }
}

#[test]
fn position_map_should_use_affinity_at_a_pure_deletion_point() {
    let map = PositionMap::from_edits(&[Edit::delete(range(1, 3))]);

    assert_eq!(
        map.map_new_position_with_affinity(b(1), Affinity::Before),
        MappingResult::Mapped(b(1))
    );
    assert_eq!(
        map.map_new_position_with_affinity(b(1), Affinity::After),
        MappingResult::Mapped(b(3))
    );
    for (new, old) in [(0, 0), (2, 4)] {
        assert_eq!(
            map.map_new_position_with_affinity(b(new), Affinity::After),
            MappingResult::Mapped(b(old))
        );
    }
}
