use super::*;
use crate::{Point as TerminalPoint, alacritty::AlacrittyCell};

fn cells(text: &str) -> Vec<IndexedCell> {
    text.chars()
        .enumerate()
        .map(|(column, character)| IndexedCell {
            point: TerminalPoint { line: 0, column },
            cell: Cell::new(AlacrittyCell {
                c: character,
                ..Default::default()
            }),
        })
        .collect()
}

#[test]
fn ime_preview_inserts_columns_at_cursor() {
    let cells = cells("abcd");
    assert_eq!(
        ime_row_parts(&cells, 0, 0, 0, ime_text_width("kaifazhe"))
            .into_iter()
            .flatten()
            .map(|(cells, shift)| (cells.first().map(|cell| cell.point.column), shift))
            .collect::<Vec<_>>(),
        vec![(Some(0), 8)]
    );
    assert_eq!(
        ime_row_parts(&cells, 0, 0, 2, ime_text_width("kaifazhe"))
            .into_iter()
            .flatten()
            .map(|(cells, shift)| (cells.first().map(|cell| cell.point.column), shift))
            .collect::<Vec<_>>(),
        vec![(Some(0), 0), (Some(2), 8)]
    );
}

#[test]
fn ime_preview_uses_terminal_width_for_wide_characters() {
    assert_eq!(ime_text_width("kaifazhe"), 8);
    assert_eq!(ime_text_width("中"), 2);
    assert_eq!(ime_text_width("e\u{301}"), 1);
}
