use std::num::NonZeroUsize;

use zom_engine::*;

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn buffer_with_tab_width(text: &str, tab_width: usize) -> Buffer {
    let config = BufferConfig {
        tab: TabConfig::new(
            NonZeroUsize::new(tab_width).unwrap(),
            NonZeroUsize::new(tab_width).unwrap(),
            true,
        ),
        ..BufferConfig::default()
    };

    Buffer::from_text(text.to_string(), config).unwrap()
}

#[test]
fn next_tab_stop_uses_configured_tab_width() {
    let buffer = buffer_with_tab_width("", 4);

    assert_eq!(
        buffer.next_tab_stop(DisplayColumn::new(0)),
        DisplayColumn::new(4)
    );
    assert_eq!(
        buffer.next_tab_stop(DisplayColumn::new(1)),
        DisplayColumn::new(4)
    );
    assert_eq!(
        buffer.next_tab_stop(DisplayColumn::new(4)),
        DisplayColumn::new(8)
    );

    let buffer = buffer_with_tab_width("", 2);
    assert_eq!(
        buffer.next_tab_stop(DisplayColumn::new(1)),
        DisplayColumn::new(2)
    );
    assert_eq!(
        buffer.next_tab_stop(DisplayColumn::new(2)),
        DisplayColumn::new(4)
    );
}

#[test]
fn logical_column_to_display_column_expands_tabs() {
    let buffer = buffer_with_tab_width("ab\tc", 4);

    assert_eq!(
        buffer
            .logical_to_display_column(Line::ZERO, LogicalColumn::new(0))
            .unwrap(),
        DisplayColumn::new(0)
    );
    assert_eq!(
        buffer
            .logical_to_display_column(Line::ZERO, LogicalColumn::new(1))
            .unwrap(),
        DisplayColumn::new(1)
    );
    assert_eq!(
        buffer
            .logical_to_display_column(Line::ZERO, LogicalColumn::new(2))
            .unwrap(),
        DisplayColumn::new(2)
    );
    assert_eq!(
        buffer
            .logical_to_display_column(Line::ZERO, LogicalColumn::new(3))
            .unwrap(),
        DisplayColumn::new(4)
    );
    assert_eq!(
        buffer
            .logical_to_display_column(Line::ZERO, LogicalColumn::new(4))
            .unwrap(),
        DisplayColumn::new(5)
    );
}

#[test]
fn display_column_to_logical_column_has_explicit_tab_affinity() {
    let buffer = buffer_with_tab_width("ab\tc", 4);

    // The tab spans display columns 2..4. Column 3 is inside the tab expansion.
    assert_eq!(
        buffer
            .display_to_logical_column_with_affinity(
                Line::ZERO,
                DisplayColumn::new(3),
                DisplayColumnAffinity::Previous,
            )
            .unwrap(),
        LogicalColumn::new(2)
    );
    assert_eq!(
        buffer
            .display_to_logical_column_with_affinity(
                Line::ZERO,
                DisplayColumn::new(3),
                DisplayColumnAffinity::Next,
            )
            .unwrap(),
        LogicalColumn::new(3)
    );
    assert_eq!(
        buffer
            .display_to_logical_column(Line::ZERO, DisplayColumn::new(3))
            .unwrap(),
        LogicalColumn::new(2)
    );
}

#[test]
fn display_column_to_char_clamps_to_line_end_when_target_is_past_content() {
    let buffer = buffer("abc\ndef");

    assert_eq!(
        buffer
            .display_to_logical_column(Line::ZERO, DisplayColumn::new(99))
            .unwrap(),
        LogicalColumn::new(3)
    );
    assert_eq!(
        buffer
            .display_column_to_char(Line::ZERO, DisplayColumn::new(99))
            .unwrap(),
        CharOffset::new(3)
    );
}

#[test]
fn cjk_and_emoji_widths_follow_display_width_policy() {
    let buffer = buffer("a中😀b");

    assert_eq!(
        buffer.char_to_display_column(CharOffset::new(0)).unwrap(),
        DisplayColumn::new(0)
    );
    assert_eq!(
        buffer.char_to_display_column(CharOffset::new(1)).unwrap(),
        DisplayColumn::new(1)
    );
    assert_eq!(
        buffer.char_to_display_column(CharOffset::new(2)).unwrap(),
        DisplayColumn::new(3)
    );
    assert_eq!(
        buffer.char_to_display_column(CharOffset::new(3)).unwrap(),
        DisplayColumn::new(5)
    );
    assert_eq!(
        buffer.char_to_display_column(CharOffset::new(4)).unwrap(),
        DisplayColumn::new(6)
    );
}

#[test]
fn combining_marks_have_zero_default_display_width() {
    let buffer = buffer("e\u{301}x");

    assert_eq!(
        buffer.char_to_display_column(CharOffset::new(0)).unwrap(),
        DisplayColumn::new(0)
    );
    assert_eq!(
        buffer.char_to_display_column(CharOffset::new(1)).unwrap(),
        DisplayColumn::new(1)
    );
    assert_eq!(
        buffer.char_to_display_column(CharOffset::new(2)).unwrap(),
        DisplayColumn::new(1)
    );
    assert_eq!(
        buffer.char_to_display_column(CharOffset::new(3)).unwrap(),
        DisplayColumn::new(2)
    );
}

#[test]
fn display_width_math_is_per_line_and_ignores_line_endings() {
    let buffer = buffer_with_tab_width("a\t\r\n中", 4);

    assert_eq!(
        buffer
            .logical_to_display_column(Line::ZERO, LogicalColumn::new(2))
            .unwrap(),
        DisplayColumn::new(4)
    );
    assert_eq!(
        buffer
            .logical_to_display_column(Line::new(1), LogicalColumn::new(1))
            .unwrap(),
        DisplayColumn::new(2)
    );
}

#[test]
fn invalid_logical_position_still_returns_coordinate_error() {
    let buffer = buffer("abc");

    assert!(
        buffer
            .logical_to_display_column(Line::ZERO, LogicalColumn::new(4))
            .is_err()
    );
    assert!(
        buffer
            .display_to_logical_column(Line::new(9), DisplayColumn::new(0))
            .is_err()
    );
}

#[test]
fn snapshot_preserves_display_width_policy_and_text() {
    let mut buffer = buffer_with_tab_width("a\tb", 4);
    let snapshot = buffer.snapshot();

    buffer.insert(CharOffset::ZERO, "xx").unwrap();

    assert_eq!(
        snapshot
            .logical_to_display_column(Line::ZERO, LogicalColumn::new(2))
            .unwrap(),
        DisplayColumn::new(4)
    );
    assert_eq!(snapshot.text().as_ref(), "a\tb");
}
