use super::{WrapEdit, WrapPatch};
use std::ops::Range;

fn edit(old: Range<usize>, new: Range<usize>) -> WrapEdit {
    WrapEdit { old, new }
}

fn compose(old: Vec<WrapEdit>, next: Vec<WrapEdit>) -> Vec<WrapEdit> {
    WrapPatch::new(old).compose(next).into_inner()
}

#[test]
fn compose_disjoint_before() {
    assert_eq!(
        compose(vec![edit(1..3, 1..4)], vec![edit(0..0, 0..4)]),
        vec![edit(0..0, 0..4), edit(1..3, 5..8)],
    );
}

#[test]
fn compose_disjoint_after() {
    assert_eq!(
        compose(vec![edit(1..3, 1..4)], vec![edit(5..9, 5..7)]),
        vec![edit(1..3, 1..4), edit(4..8, 5..7)],
    );
}

#[test]
fn compose_overlapping() {
    assert_eq!(
        compose(vec![edit(1..3, 1..4)], vec![edit(3..5, 3..6)]),
        vec![edit(1..4, 1..6)],
    );
}

#[test]
fn compose_two_disjoint_and_overlapping() {
    assert_eq!(
        compose(
            vec![edit(1..3, 1..4), edit(8..12, 9..11)],
            vec![edit(0..0, 0..4), edit(3..10, 7..9)],
        ),
        vec![edit(0..0, 0..4), edit(1..12, 5..10)],
    );
}
