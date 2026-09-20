use crate::{Buffer, BufferConfig, Edit, TransactionMetadata};

use super::*;

fn patch(edits: &[(Range<usize>, Range<usize>)]) -> TextPatch {
    TextPatch {
        edits: edits
            .iter()
            .cloned()
            .map(|(old, new)| PatchEdit::new(old, new))
            .collect(),
    }
}

#[test]
fn composing_disjoint_and_overlapping_patches_preserves_outer_coordinates() {
    let first = patch(&[(1..3, 1..4)]);
    let disjoint = patch(&[(5..9, 5..7)]);
    assert_eq!(
        first.compose(&disjoint),
        patch(&[(1..3, 1..4), (4..8, 5..7)])
    );

    let overlapping = patch(&[(3..5, 3..6)]);
    assert_eq!(first.compose(&overlapping), patch(&[(1..4, 1..6)]));
}

#[test]
fn subscriptions_are_independent_and_compose_continuous_updates() {
    let mut buffer =
        Buffer::from_text("abc".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
    let first = buffer.subscribe();
    let second = buffer.subscribe();
    let initial_version = buffer.version();

    for replacement in ["x", "yz"] {
        buffer
            .edit(
                [Edit::insert(buffer.len_bytes(), replacement).expect("插入应有效")],
                TransactionMetadata::default(),
            )
            .expect("事务应成功");
    }

    let first_batch = first.consume();
    assert_eq!(first_batch.old_version(), Some(initial_version));
    assert_eq!(first_batch.new_version(), Some(buffer.version()));
    assert!(!first_batch.patch().is_empty());
    assert_eq!(
        first_batch.position_map(),
        PositionMap::from_text_patch(first_batch.patch())
    );
    assert!(first.consume().is_empty());

    let second_batch = second.consume();
    assert_eq!(second_batch.patch(), first_batch.patch());
}

#[test]
fn composition_collapses_edits_to_inserted_and_original_text_into_one_outer_edit() {
    let insert_before_original = patch(&[(0..0, 0..1)]);
    let delete_original = patch(&[(1..2, 1..1)]);

    assert_eq!(
        insert_before_original.compose(&delete_original),
        patch(&[(0..1, 0..1)])
    );
}
