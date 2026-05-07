//! M9 机器契约：聚合 Anchor / Mark、TrackedRange、Selection 与外部 range 映射。
//!
//! 小阶段测试保留在本文件的子模块中，避免一个大阶段拆出多个 cargo test 入口。

mod m9a_anchor_mark {
    //! M9A：锁定 Anchor / Mark 的版本绑定、Affinity 与 PositionMap 跟随语义。

    use zom_engine::{
        Affinity, Anchor, AnchorDeletedPolicy, AnchorError, AnchorUpdate, Buffer, BufferConfig,
        BufferVersion, CharOffset, Edit, MappingResult, Mark, TextRange, Transaction,
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

    fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> zom_engine::DeltaEvent {
        let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
        buffer.apply_transaction(tx).unwrap();
        buffer.last_delta_event().unwrap().clone()
    }

    #[test]
    fn anchor_binds_position_to_buffer_version_and_affinity() {
        let buffer = buffer("ab");
        let anchor = Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::Before);

        assert_eq!(anchor.version(), BufferVersion::INITIAL);
        assert_eq!(anchor.offset(), c(1));
        assert_eq!(anchor.affinity(), Affinity::Before);
        assert_eq!(
            anchor.to_mark(),
            Mark::new(c(1)).with_affinity(Affinity::Before)
        );
    }

    #[test]
    fn anchor_maps_through_delta_event_with_affinity() {
        let mut buffer = buffer("ab");
        let before = Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::Before);
        let after = Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::After);

        let event = apply(
            &mut buffer,
            vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
        );

        assert_eq!(
            before.map_through_delta_event(&event),
            Ok(MappingResult::Mapped(
                Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::Before)
            ))
        );
        assert_eq!(
            after.map_through_delta_event(&event),
            Ok(MappingResult::Mapped(
                Anchor::new(buffer.version(), c(4)).with_affinity(Affinity::After)
            ))
        );
    }

    #[test]
    fn mark_maps_as_lightweight_unversioned_position() {
        let mut buffer = buffer("ab");
        let event = apply(
            &mut buffer,
            vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
        );

        let before = Mark::new(c(1)).with_affinity(Affinity::Before);
        let after = Mark::new(c(1)).with_affinity(Affinity::After);

        assert_eq!(
            before.map_through_position_map(&event.position_map),
            MappingResult::Mapped(Mark::new(c(1)).with_affinity(Affinity::Before))
        );
        assert_eq!(
            after.map_through_position_map(&event.position_map),
            MappingResult::Mapped(Mark::new(c(4)).with_affinity(Affinity::After))
        );
    }

    #[test]
    fn anchor_rejects_mapping_through_unrelated_version_event() {
        let mut buffer = buffer("abc");
        let stale = Anchor::new(BufferVersion::new(99), c(1));
        let event = apply(&mut buffer, vec![Edit::delete(range(0, 1))]);

        assert_eq!(
            stale.map_through_delta_event(&event),
            Err(AnchorError::VersionMismatch {
                expected: BufferVersion::INITIAL,
                actual: BufferVersion::new(99),
            })
        );
    }

    #[test]
    fn deleted_anchor_can_collapse_or_invalidate() {
        let mut buffer = buffer("abcdef");
        let anchor = Anchor::new(buffer.version(), c(2));
        let event = apply(&mut buffer, vec![Edit::delete(range(1, 4))]);

        assert_eq!(
            anchor.map_through_delta_event(&event),
            Ok(MappingResult::Deleted(Anchor::new(buffer.version(), c(1))))
        );
        assert_eq!(
            anchor
                .map_through_delta_event_with_deleted_policy(&event, AnchorDeletedPolicy::Collapse),
            Ok(AnchorUpdate::Deleted(Anchor::new(buffer.version(), c(1))))
        );
        assert_eq!(
            anchor.map_through_delta_event_with_deleted_policy(
                &event,
                AnchorDeletedPolicy::Invalidate
            ),
            Ok(AnchorUpdate::Invalidated {
                mark: Mark::new(c(1)),
                version: buffer.version(),
            })
        );
    }

    #[test]
    fn anchors_can_be_updated_in_batch_without_partial_version_mismatch_mutation() {
        let mut buffer = buffer("abc");
        let mut anchors = vec![
            Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::Before),
            Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::After),
        ];
        let event = apply(
            &mut buffer,
            vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
        );

        let updates = Anchor::update_all_through_delta_event(&mut anchors, &event).unwrap();

        assert_eq!(
            updates,
            vec![
                MappingResult::Mapped(
                    Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::Before)
                ),
                MappingResult::Mapped(
                    Anchor::new(buffer.version(), c(4)).with_affinity(Affinity::After)
                ),
            ]
        );
        assert_eq!(
            anchors,
            vec![
                Anchor::new(buffer.version(), c(1)).with_affinity(Affinity::Before),
                Anchor::new(buffer.version(), c(4)).with_affinity(Affinity::After),
            ]
        );

        let before_failed_update = anchors.clone();
        let mut other_buffer =
            Buffer::from_text("xyz".to_string(), BufferConfig::default()).unwrap();
        let unrelated_event = apply(&mut other_buffer, vec![Edit::delete(range(0, 1))]);
        let err =
            Anchor::update_all_through_delta_event(&mut anchors, &unrelated_event).unwrap_err();

        assert_eq!(
            err,
            AnchorError::VersionMismatch {
                expected: BufferVersion::INITIAL,
                actual: BufferVersion::new(1),
            }
        );
        assert_eq!(anchors, before_failed_update);
    }

    #[test]
    fn anchors_can_be_batch_mapped_with_deleted_policy() {
        let mut buffer = buffer("abcdef");
        let anchors = vec![
            Anchor::new(buffer.version(), c(2)),
            Anchor::new(buffer.version(), c(5)),
        ];
        let event = apply(&mut buffer, vec![Edit::delete(range(1, 4))]);

        let updates = Anchor::map_all_through_delta_event_with_deleted_policy(
            anchors,
            &event,
            AnchorDeletedPolicy::Invalidate,
        )
        .unwrap();

        assert_eq!(
            updates,
            vec![
                AnchorUpdate::Invalidated {
                    mark: Mark::new(c(1)),
                    version: buffer.version(),
                },
                AnchorUpdate::Mapped(Anchor::new(buffer.version(), c(2))),
            ]
        );
    }
}

mod m9b_tracked_range {
    //! M9B：锁定 TrackedRange 的 Anchor 边界、stickiness、失效与批量更新语义。

    use zom_engine::{
        Affinity, Anchor, AnchorError, Buffer, BufferConfig, BufferVersion, CharOffset, Edit,
        EngineError, MappingResult, Stickiness, TextRange, TrackedRange,
        TrackedRangeCollapsePolicy, TrackedRangeInvalidationPolicy, TrackedRangeUpdate,
        TrackedRangeUpdatePolicy, Transaction,
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

    fn tracked(
        version: BufferVersion,
        start: usize,
        end: usize,
        stickiness: Stickiness,
    ) -> TrackedRange {
        TrackedRange::from_range(version, range(start, end), stickiness)
    }

    fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> zom_engine::DeltaEvent {
        let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
        buffer.apply_transaction(tx).unwrap();
        buffer.last_delta_event().unwrap().clone()
    }

    #[test]
    fn tracked_range_is_expressed_by_two_versioned_anchors() {
        let tracked = tracked(BufferVersion::INITIAL, 1, 3, Stickiness::Expand);

        assert_eq!(tracked.version(), BufferVersion::INITIAL);
        assert_eq!(tracked.range(), range(1, 3));
        assert_eq!(tracked.stickiness(), Stickiness::Expand);
        assert_eq!(tracked.start_anchor().offset(), c(1));
        assert_eq!(tracked.start_anchor().affinity(), Affinity::Before);
        assert_eq!(tracked.end_anchor().offset(), c(3));
        assert_eq!(tracked.end_anchor().affinity(), Affinity::After);
    }

    #[test]
    fn tracked_range_constructor_rejects_mismatched_anchor_versions_and_reversed_range() {
        let err = TrackedRange::new(
            Anchor::new(BufferVersion::INITIAL, c(1)),
            Anchor::new(BufferVersion::new(1), c(3)),
            Stickiness::Never,
        )
        .unwrap_err();

        assert_eq!(
            err,
            EngineError::Anchor(AnchorError::RangeVersionMismatch {
                start: BufferVersion::INITIAL,
                end: BufferVersion::new(1),
            })
        );

        let err = TrackedRange::new(
            Anchor::new(BufferVersion::INITIAL, c(3)),
            Anchor::new(BufferVersion::INITIAL, c(1)),
            Stickiness::Never,
        )
        .unwrap_err();

        assert!(matches!(err, EngineError::Coordinate(_)));
    }

    #[test]
    fn stickiness_controls_growth_at_insert_boundaries() {
        let mut buffer = buffer("abcd");
        let version = buffer.version();
        let event = apply(
            &mut buffer,
            vec![
                Edit::insert(c(1), "X".to_string()).unwrap(),
                Edit::insert(c(3), "Y".to_string()).unwrap(),
            ],
        );

        let never = tracked(version, 1, 3, Stickiness::Never);
        let expand = tracked(version, 1, 3, Stickiness::Expand);
        let before = tracked(version, 1, 3, Stickiness::BeforeInsertion);
        let after = tracked(version, 1, 3, Stickiness::AfterInsertion);

        assert_eq!(
            never.map_through_delta_event(&event),
            Ok(MappingResult::Mapped(tracked(
                buffer.version(),
                2,
                4,
                Stickiness::Never
            )))
        );
        assert_eq!(
            expand.map_through_delta_event(&event),
            Ok(MappingResult::Mapped(tracked(
                buffer.version(),
                1,
                5,
                Stickiness::Expand
            )))
        );
        assert_eq!(
            before.map_through_delta_event(&event),
            Ok(MappingResult::Mapped(tracked(
                buffer.version(),
                1,
                4,
                Stickiness::BeforeInsertion
            )))
        );
        assert_eq!(
            after.map_through_delta_event(&event),
            Ok(MappingResult::Mapped(tracked(
                buffer.version(),
                2,
                5,
                Stickiness::AfterInsertion
            )))
        );
    }

    #[test]
    fn fully_deleted_range_can_collapse_or_invalidate() {
        let mut buffer = buffer("abcdef");
        let tracked_range = tracked(buffer.version(), 1, 4, Stickiness::Never);
        let event = apply(&mut buffer, vec![Edit::delete(range(1, 4))]);

        assert_eq!(
            tracked_range.map_through_delta_event(&event),
            Ok(MappingResult::Collapsed(tracked(
                buffer.version(),
                1,
                1,
                Stickiness::Never
            )))
        );
        assert_eq!(
            tracked_range.map_through_delta_event_with_policy(
                &event,
                TrackedRangeUpdatePolicy::invalidate_when_fully_deleted()
            ),
            Ok(TrackedRangeUpdate::Invalidated {
                range: range(1, 1),
                version: buffer.version(),
            })
        );
        assert_eq!(
            tracked_range.map_through_delta_event_with_policy(
                &event,
                TrackedRangeUpdatePolicy::new(
                    TrackedRangeInvalidationPolicy::Never,
                    TrackedRangeCollapsePolicy::Invalidate,
                )
            ),
            Ok(TrackedRangeUpdate::Invalidated {
                range: range(1, 1),
                version: buffer.version(),
            })
        );
    }

    #[test]
    fn partially_deleted_range_shrinks_by_default_or_invalidates_when_requested() {
        let mut buffer = buffer("abcdef");
        let tracked_range = tracked(buffer.version(), 1, 5, Stickiness::Never);
        let event = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);

        assert_eq!(buffer.text().as_ref(), "abef");
        assert_eq!(
            tracked_range.map_through_delta_event(&event),
            Ok(MappingResult::Deleted(tracked(
                buffer.version(),
                1,
                3,
                Stickiness::Never
            )))
        );
        assert_eq!(
            tracked_range
                .map_through_delta_event_with_policy(&event, TrackedRangeUpdatePolicy::default()),
            Ok(TrackedRangeUpdate::Deleted(tracked(
                buffer.version(),
                1,
                3,
                Stickiness::Never
            )))
        );
        assert_eq!(
            tracked_range.map_through_delta_event_with_policy(
                &event,
                TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion()
            ),
            Ok(TrackedRangeUpdate::Invalidated {
                range: range(1, 3),
                version: buffer.version(),
            })
        );
    }

    #[test]
    fn tracked_ranges_can_be_updated_in_batch_without_partial_version_mismatch_mutation() {
        let mut buffer = buffer("abcdef");
        let mut ranges = vec![
            tracked(buffer.version(), 1, 3, Stickiness::Never),
            tracked(buffer.version(), 3, 6, Stickiness::Expand),
        ];
        let event = apply(
            &mut buffer,
            vec![Edit::insert(c(3), "XYZ".to_string()).unwrap()],
        );

        let updates = TrackedRange::update_all_through_delta_event(&mut ranges, &event).unwrap();

        assert_eq!(
            updates,
            vec![
                MappingResult::Mapped(tracked(buffer.version(), 1, 3, Stickiness::Never)),
                MappingResult::Mapped(tracked(buffer.version(), 3, 9, Stickiness::Expand)),
            ]
        );
        assert_eq!(
            ranges,
            vec![
                tracked(buffer.version(), 1, 3, Stickiness::Never),
                tracked(buffer.version(), 3, 9, Stickiness::Expand),
            ]
        );

        let before_failed_update = ranges.clone();
        let mut other_buffer =
            Buffer::from_text("xyz".to_string(), BufferConfig::default()).unwrap();
        let unrelated_event = apply(&mut other_buffer, vec![Edit::delete(range(0, 1))]);
        let err = TrackedRange::update_all_through_delta_event(&mut ranges, &unrelated_event)
            .unwrap_err();

        assert_eq!(
            err,
            AnchorError::VersionMismatch {
                expected: BufferVersion::INITIAL,
                actual: BufferVersion::new(1),
            }
        );
        assert_eq!(ranges, before_failed_update);
    }
}

mod m9c_selection_external_range {
    //! M9C：锁定 Selection 与外部 range 通过 PositionMap 跟随文本变化。

    use zom_engine::{
        Buffer, BufferConfig, BufferVersion, CharOffset, Edit, MappingResult, Selection,
        SelectionSet, Stickiness, TextRange, TrackedRange, TrackedRangeInvalidationPolicy,
        TrackedRangeUpdate, TrackedRangeUpdatePolicy, Transaction,
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

    fn tracked(
        version: BufferVersion,
        start: usize,
        end: usize,
        stickiness: Stickiness,
    ) -> TrackedRange {
        TrackedRange::from_range(version, range(start, end), stickiness)
    }

    fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> zom_engine::DeltaEvent {
        let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
        buffer.apply_transaction(tx).unwrap();
        buffer.last_delta_event().unwrap().clone()
    }

    #[test]
    fn position_map_maps_selection_without_losing_anchor_head_direction() {
        let mut buffer = buffer("abcdef");
        let selection = Selection::new(c(4), c(1));
        let event = apply(
            &mut buffer,
            vec![Edit::insert(c(2), "XYZ".to_string()).unwrap()],
        );

        let mapped = event.position_map.map_selection(selection);

        assert_eq!(mapped, Selection::new(c(7), c(1)));
        assert!(mapped.is_reversed());
        assert_eq!(
            mapped,
            selection.map_through_position_map(&event.position_map)
        );
    }

    #[test]
    fn position_map_maps_selection_set_and_preserves_primary_selection() {
        let mut buffer = buffer("abcdef");
        let selection_set = SelectionSet::new_with_primary(
            vec![Selection::new(c(1), c(2)), Selection::caret(c(4))],
            1,
        );
        let event = apply(
            &mut buffer,
            vec![Edit::insert(c(0), "X".to_string()).unwrap()],
        );

        let mapped = event.position_map.map_selection_set(&selection_set);

        assert_eq!(mapped.primary_index(), 1);
        assert_eq!(
            mapped.as_slice(),
            &[Selection::new(c(2), c(3)), Selection::caret(c(5))]
        );
        assert_eq!(
            mapped,
            selection_set.map_through_position_map(&event.position_map)
        );
    }

    #[test]
    fn external_ranges_follow_through_position_map_without_metadata_business_types() {
        let mut buffer = buffer("abcdef");
        let version = buffer.version();

        let search_result_range = tracked(version, 1, 5, Stickiness::Never);
        let diagnostic_range = tracked(version, 2, 4, Stickiness::Never);
        let breakpoint_range = tracked(version, 3, 3, Stickiness::Expand);
        let bookmark_range = tracked(version, 5, 5, Stickiness::Never);

        let event = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);
        assert_eq!(buffer.text().as_ref(), "abef");

        let updates = event.position_map.map_tracked_ranges_with_policy(
            vec![
                search_result_range,
                diagnostic_range,
                breakpoint_range,
                bookmark_range,
            ],
            buffer.version(),
            TrackedRangeUpdatePolicy::default(),
        );

        assert_eq!(
            updates,
            vec![
                TrackedRangeUpdate::Deleted(tracked(buffer.version(), 1, 3, Stickiness::Never)),
                TrackedRangeUpdate::Collapsed(tracked(buffer.version(), 2, 2, Stickiness::Never)),
                TrackedRangeUpdate::Deleted(tracked(buffer.version(), 2, 2, Stickiness::Expand)),
                TrackedRangeUpdate::Mapped(tracked(buffer.version(), 3, 3, Stickiness::Never)),
            ]
        );
    }

    #[test]
    fn external_ranges_can_share_deletion_invalidation_policy() {
        let mut buffer = buffer("abcdef");
        let version = buffer.version();

        let search_result_range = tracked(version, 1, 5, Stickiness::Never);
        let diagnostic_range = tracked(version, 2, 4, Stickiness::Never);
        let bookmark_range = tracked(version, 5, 5, Stickiness::Never);

        let event = apply(&mut buffer, vec![Edit::delete(range(2, 4))]);
        let updates = event.position_map.map_tracked_ranges_with_policy(
            vec![search_result_range, diagnostic_range, bookmark_range],
            buffer.version(),
            TrackedRangeUpdatePolicy::new(
                TrackedRangeInvalidationPolicy::WhenTouchedByDeletion,
                zom_engine::TrackedRangeCollapsePolicy::Keep,
            ),
        );

        assert_eq!(
            updates,
            vec![
                TrackedRangeUpdate::Invalidated {
                    range: range(1, 3),
                    version: buffer.version(),
                },
                TrackedRangeUpdate::Invalidated {
                    range: range(2, 2),
                    version: buffer.version(),
                },
                TrackedRangeUpdate::Mapped(tracked(buffer.version(), 3, 3, Stickiness::Never)),
            ]
        );
    }

    #[test]
    fn position_map_can_map_single_tracked_range_for_external_consumers() {
        let mut buffer = buffer("abcd");
        let external_range = tracked(buffer.version(), 1, 3, Stickiness::Expand);
        let event = apply(
            &mut buffer,
            vec![
                Edit::insert(c(1), "X".to_string()).unwrap(),
                Edit::insert(c(3), "Y".to_string()).unwrap(),
            ],
        );

        assert_eq!(
            event
                .position_map
                .map_tracked_range(external_range, buffer.version()),
            MappingResult::Mapped(tracked(buffer.version(), 1, 5, Stickiness::Expand))
        );
    }
}
