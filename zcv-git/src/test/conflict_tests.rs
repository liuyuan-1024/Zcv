use super::*;

const TEXT: &str = "before\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\nafter\n";

#[test]
fn resolves_each_side_without_leaving_markers() {
    let conflict = &parse_conflict_regions(TEXT)[0];
    assert_eq!(
        resolve_conflict(TEXT, conflict, ConflictChoice::Ours),
        "before\nours\nafter\n"
    );
    assert_eq!(
        resolve_conflict(TEXT, conflict, ConflictChoice::Theirs),
        "before\ntheirs\nafter\n"
    );
    assert_eq!(
        resolve_conflict(TEXT, conflict, ConflictChoice::Both),
        "before\nours\ntheirs\nafter\n"
    );
}
