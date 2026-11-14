use super::*;

#[test]
fn disabled_state_uses_not_allowed_cursor() {
    assert_eq!(
        cursor_for_state(true, true),
        Some(CursorStyle::OperationNotAllowed)
    );
    assert_eq!(
        cursor_for_state(true, false),
        Some(CursorStyle::OperationNotAllowed)
    );
}

#[test]
fn enabled_state_preserves_existing_interaction() {
    assert_eq!(
        cursor_for_state(false, true),
        Some(CursorStyle::PointingHand)
    );
    assert_eq!(cursor_for_state(false, false), None);
}
