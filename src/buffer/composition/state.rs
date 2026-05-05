use crate::{CharOffset, CompositionSelection, EngineResult, Selection, TextRange};

pub(in crate::buffer) fn resolve_relative_selection(
    selection: Option<CompositionSelection>,
    preedit_len: usize,
) -> CompositionSelection {
    selection.unwrap_or_else(|| CompositionSelection::caret(CharOffset::new(preedit_len)))
}

pub(in crate::buffer) fn absolute_composition_selection(
    range_start: CharOffset,
    selection: CompositionSelection,
) -> EngineResult<Selection> {
    Ok(Selection::new(
        CharOffset::new(range_start.get() + selection.anchor().get()),
        CharOffset::new(range_start.get() + selection.head().get()),
    ))
}

pub(in crate::buffer) fn composition_range_after_preedit(
    range_start: CharOffset,
    preedit_len: usize,
) -> EngineResult<TextRange> {
    let range_end = CharOffset::new(range_start.get() + preedit_len);
    Ok(TextRange::new(range_start, range_end)?)
}
