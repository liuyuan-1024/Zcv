use super::*;
use crate::{ByteOffset, Edit};

fn append(log: &EditLog, old: u64, replacement: &str, undo: Option<&str>) -> EditLog {
    let forward =
        EditList::new(vec![Edit::insert(ByteOffset::new(0), replacement).unwrap()]).unwrap();
    let undo = undo
        .map(|text| EditList::new(vec![Edit::insert(ByteOffset::new(0), text).unwrap()]).unwrap());
    log.appended(
        BufferVersion::new(old),
        BufferVersion::new(old + 1),
        forward,
        undo,
    )
}

#[test]
fn old_log_remains_readable_after_append_and_count_trimming() {
    let log = append(&EditLog::default(), 0, "a", None);
    let retained = append(&log, 1, "b", None);
    let advanced = append(&retained, 2, "c", None);
    let trimmed = advanced.truncated(2, 0);

    assert!(
        retained
            .batch_since(BufferVersion::new(0), BufferVersion::new(2))
            .is_ok()
    );
    assert!(matches!(
        retained.batch_since(BufferVersion::new(0), BufferVersion::new(3)),
        Err(TextError::VersionEvicted { .. })
    ));
    assert_eq!(trimmed.earliest_version(), Some(BufferVersion::new(1)));
    assert!(
        trimmed
            .batch_since(BufferVersion::new(1), BufferVersion::new(3))
            .is_ok()
    );
    assert!(matches!(
        trimmed.batch_since(BufferVersion::new(0), BufferVersion::new(3)),
        Err(TextError::VersionEvicted { .. })
    ));
}

#[test]
fn byte_budget_counts_forward_and_undo_and_keeps_latest_entry() {
    let first = append(&EditLog::default(), 0, "a", None);
    let second = append(&first, 1, "b", Some("old"));
    let third = append(&second, 2, "ccc", None);

    let within_budget = third.truncated(10, 7);
    assert_eq!(
        within_budget.earliest_version(),
        Some(BufferVersion::new(1))
    );

    let latest_only = third.truncated(10, 1);
    assert_eq!(latest_only.earliest_version(), Some(BufferVersion::new(2)));
    assert!(
        latest_only
            .batch_since(BufferVersion::new(2), BufferVersion::new(3))
            .is_ok()
    );

    assert_eq!(third.truncated(0, 0).earliest_version(), None);
    assert_eq!(first.earliest_version(), Some(BufferVersion::new(0)));
}

#[test]
fn long_history_trims_prefix_and_replays_in_both_directions() {
    let mut log = EditLog::default();
    for version in 0..130 {
        log = append(&log, version, "x", Some("u"));
    }
    let old = log.clone();
    let trimmed = log.truncated(64, 0);

    assert_eq!(trimmed.earliest_version(), Some(BufferVersion::new(66)));
    assert!(
        trimmed
            .batch_since(BufferVersion::new(66), BufferVersion::new(130))
            .is_ok()
    );
    assert!(
        old.batch_since(BufferVersion::new(0), BufferVersion::new(130))
            .is_ok()
    );

    let undo = trimmed
        .undo_batches(BufferVersion::new(66), BufferVersion::new(130))
        .unwrap();
    let redo = trimmed
        .redo_batches(BufferVersion::new(66), BufferVersion::new(130))
        .unwrap();
    assert_eq!(undo.len(), 64);
    assert_eq!(undo.first().unwrap().0, BufferVersion::new(129));
    assert_eq!(undo.last().unwrap().0, BufferVersion::new(66));
    assert_eq!(redo.first().unwrap().0, BufferVersion::new(66));
    assert_eq!(redo.last().unwrap().0, BufferVersion::new(129));
}
