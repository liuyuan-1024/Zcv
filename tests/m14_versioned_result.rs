//! M14A 机器契约：锁定 `VersionedResult<T>` 的版本绑定、过期判断、过期丢弃 helper、
//! 与通过 `PositionMap` / `DeltaEvent` 的 remap 行为。

use std::cell::Cell;

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, DeltaEvent, Edit, EngineError, MappingResult,
    Stickiness, TextRange, Transaction, VersionedResult, VersionedResultError,
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

fn apply(buffer: &mut Buffer, edits: Vec<Edit>) -> DeltaEvent {
    let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
    buffer.apply_transaction(tx).unwrap();
    buffer.last_delta_event().unwrap().clone()
}

#[test]
fn new_binds_buffer_version() {
    let result = VersionedResult::new(BufferVersion::new(3), 42usize);

    assert_eq!(result.version(), BufferVersion::new(3));
    assert_eq!(*result.value(), 42);
    assert_eq!(result.into_parts(), (BufferVersion::new(3), 42));
}

#[test]
fn is_stale_when_version_differs() {
    let result = VersionedResult::new(BufferVersion::new(5), "payload");

    assert!(!result.is_stale(BufferVersion::new(5)));
    assert!(result.is_stale(BufferVersion::new(6)));
    assert!(result.is_stale(BufferVersion::new(4)));
}

#[test]
fn is_stale_false_for_initial_version_match() {
    let result = VersionedResult::new(BufferVersion::INITIAL, ());

    assert!(!result.is_stale(BufferVersion::INITIAL));
}

#[test]
fn discard_if_stale_returns_none_when_stale() {
    let result = VersionedResult::new(BufferVersion::new(2), 7u32);

    assert!(result.discard_if_stale(BufferVersion::new(3)).is_none());
}

#[test]
fn discard_if_stale_returns_some_when_current() {
    let result = VersionedResult::new(BufferVersion::new(2), 7u32);

    let kept = result.discard_if_stale(BufferVersion::new(2)).unwrap();
    assert_eq!(kept.version(), BufferVersion::new(2));
    assert_eq!(*kept.value(), 7);
}

#[test]
fn map_preserves_version() {
    let result = VersionedResult::new(BufferVersion::new(9), 4u32);
    let mapped = result.map(|value| value + 1);

    assert_eq!(mapped.version(), BufferVersion::new(9));
    assert_eq!(*mapped.value(), 5);
}

#[test]
fn try_remap_rejects_event_with_wrong_old_version() {
    let mut text = buffer("ab");
    let event = apply(
        &mut text,
        vec![Edit::insert(c(1), "X".to_string()).unwrap()],
    );

    // 把 result 绑定到与 event.old_version 不一致的版本。
    let stale_version = BufferVersion::new(event.old_version.get() + 7);
    let result = VersionedResult::new(stale_version, c(1));
    let called = Cell::new(false);

    let outcome = result.try_remap(&event, |_value, _map| {
        called.set(true);
        Ok(c(0))
    });

    match outcome {
        Err(EngineError::Versioned(VersionedResultError::VersionMismatch { expected, actual })) => {
            assert_eq!(expected, stale_version);
            assert_eq!(actual, event.old_version);
        }
        other => panic!("预期 VersionMismatch，实际 {other:?}"),
    }
    assert!(!called.get(), "版本不匹配时不应调用 remap 闭包");
}

#[test]
fn try_remap_advances_to_new_version_on_success() {
    let mut text = buffer("ab");
    let result = VersionedResult::new(text.version(), 0u32);
    let event = apply(
        &mut text,
        vec![Edit::insert(c(1), "X".to_string()).unwrap()],
    );

    let remapped = result
        .try_remap(&event, |value, _map| Ok(value + 1))
        .unwrap();

    assert_eq!(remapped.version(), event.new_version);
    assert_eq!(*remapped.value(), 1);
}

#[test]
fn try_remap_propagates_remap_failure() {
    let mut text = buffer("ab");
    let result = VersionedResult::new(text.version(), 99u32);
    let event = apply(
        &mut text,
        vec![Edit::insert(c(1), "X".to_string()).unwrap()],
    );

    let outcome = result.try_remap(&event, |_value, _map| {
        Err(VersionedResultError::RemapFailed {
            reason: "payload no longer mappable".to_string(),
        })
    });

    match outcome {
        Err(EngineError::Versioned(VersionedResultError::RemapFailed { reason })) => {
            assert_eq!(reason, "payload no longer mappable");
        }
        other => panic!("预期 RemapFailed，实际 {other:?}"),
    }
}

#[test]
fn try_remap_uses_position_map_for_char_offset_payload() {
    let mut text = buffer("abcd");
    let result = VersionedResult::new(text.version(), c(2));
    let event = apply(
        &mut text,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    let remapped = result
        .try_remap(&event, |offset, position_map| {
            match position_map.map_old_position(offset) {
                MappingResult::Mapped(new_offset) => Ok(new_offset),
                MappingResult::Deleted(_)
                | MappingResult::Collapsed(_)
                | MappingResult::Ambiguous(_) => Err(VersionedResultError::RemapFailed {
                    reason: "char offset lost".to_string(),
                }),
            }
        })
        .unwrap();

    assert_eq!(remapped.version(), event.new_version);
    assert_eq!(*remapped.value(), c(5));
}

#[test]
fn try_remap_uses_position_map_for_text_range_payload() {
    let mut text = buffer("abcd");
    let result = VersionedResult::new(text.version(), range(2, 4));
    let event = apply(
        &mut text,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    let remapped = result
        .try_remap(&event, |range, position_map| {
            match position_map.map_old_range_with_stickiness(range, Stickiness::Never) {
                MappingResult::Mapped(new_range) => Ok(new_range),
                MappingResult::Deleted(_)
                | MappingResult::Collapsed(_)
                | MappingResult::Ambiguous(_) => Err(VersionedResultError::RemapFailed {
                    reason: "range intersected deleted content".to_string(),
                }),
            }
        })
        .unwrap();

    assert_eq!(remapped.version(), event.new_version);
    assert_eq!(*remapped.value(), range(5, 7));
}

#[test]
fn try_remap_with_skips_version_check() {
    let mut text = buffer("abcd");
    let stale_version = BufferVersion::new(text.version().get() + 100);
    let result = VersionedResult::new(stale_version, c(2));
    let event = apply(
        &mut text,
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    );

    let success = result
        .try_remap_with(&event.position_map, event.new_version, |offset, map| {
            Ok(map.map_old_position(offset).value())
        })
        .unwrap();
    assert_eq!(success.version(), event.new_version);
    assert_eq!(*success.value(), c(5));

    let failure_result = VersionedResult::new(stale_version, c(2));
    let failure =
        failure_result.try_remap_with(&event.position_map, event.new_version, |_offset, _map| {
            Err(VersionedResultError::RemapFailed {
                reason: "explicit fail".to_string(),
            })
        });
    match failure {
        Err(EngineError::Versioned(VersionedResultError::RemapFailed { reason })) => {
            assert_eq!(reason, "explicit fail");
        }
        other => panic!("预期 RemapFailed，实际 {other:?}"),
    }
}
