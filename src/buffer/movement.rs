use unicode_segmentation::UnicodeSegmentation;

use crate::{
    CharOffset, CoordinateError, EditError, EngineResult, MovementDirection, MovementUnit,
    Selection, SelectionSet, WordBoundaryPolicy,
    storage::TextRead,
};

use super::{Buffer, coordinates::is_crlf_middle};

impl Buffer {
    /// M6B：按给定移动粒度寻找前一个边界。
    pub fn previous_movement_boundary(
        &self,
        offset: CharOffset,
        unit: MovementUnit,
    ) -> EngineResult<CharOffset> {
        self.movement_boundary(offset, MovementDirection::Previous, unit)
    }

    /// M6B：按给定移动粒度寻找后一个边界。
    pub fn next_movement_boundary(
        &self,
        offset: CharOffset,
        unit: MovementUnit,
    ) -> EngineResult<CharOffset> {
        self.movement_boundary(offset, MovementDirection::Next, unit)
    }

    /// M6B：统一移动边界入口。
    pub fn movement_boundary(
        &self,
        offset: CharOffset,
        direction: MovementDirection,
        unit: MovementUnit,
    ) -> EngineResult<CharOffset> {
        movement_boundary_in_text(
            &self.storage,
            self.config.word_boundary,
            offset,
            direction,
            unit,
        )
    }

    pub fn previous_word_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.previous_movement_boundary(offset, MovementUnit::Word)
    }

    pub fn next_word_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.next_movement_boundary(offset, MovementUnit::Word)
    }

    pub fn previous_identifier_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.previous_movement_boundary(offset, MovementUnit::Identifier)
    }

    pub fn next_identifier_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.next_movement_boundary(offset, MovementUnit::Identifier)
    }

    pub fn previous_subword_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.previous_movement_boundary(offset, MovementUnit::Subword)
    }

    pub fn next_subword_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.next_movement_boundary(offset, MovementUnit::Subword)
    }

    pub fn previous_symbol_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.previous_movement_boundary(offset, MovementUnit::Symbol)
    }

    pub fn next_symbol_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.next_movement_boundary(offset, MovementUnit::Symbol)
    }

    /// M6B：移动一组选区的 head。
    ///
    /// `extend = false` 时移动后塌缩为 caret；`extend = true` 时保留 anchor，扩展/收缩选区。
    /// 该 API 只更新 selection，不提交文本事务，因此不污染 Undo 历史。
    pub fn move_selections(
        &mut self,
        selections: SelectionSet,
        direction: MovementDirection,
        unit: MovementUnit,
        extend: bool,
    ) -> EngineResult<SelectionSet> {
        self.validate_selection_set(&selections)?;

        let primary_index = selections.primary_index();
        let moved = selections
            .as_slice()
            .iter()
            .copied()
            .map(|selection| {
                let new_head = self.movement_boundary(selection.head(), direction, unit)?;

                Ok(if extend {
                    selection.with_head(new_head)
                } else {
                    Selection::caret(new_head)
                })
            })
            .collect::<EngineResult<Vec<_>>>()?;

        let moved = SelectionSet::new_with_primary(moved, primary_index);
        self.set_selection(moved.clone())?;
        Ok(moved)
    }

    /// M6B：移动当前 Buffer selection 的便捷入口。
    pub fn move_current_selection(
        &mut self,
        direction: MovementDirection,
        unit: MovementUnit,
        extend: bool,
    ) -> EngineResult<SelectionSet> {
        let selections = self.selection.clone();
        self.move_selections(selections, direction, unit, extend)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MovementTokenSpan {
    start: CharOffset,
    end: CharOffset,
}

impl MovementTokenSpan {
    fn new(start: usize, end: usize) -> Self {
        Self {
            start: CharOffset::new(start),
            end: CharOffset::new(end),
        }
    }
}

fn movement_boundary_in_text<T: TextRead>(
    storage: &T,
    policy: WordBoundaryPolicy,
    offset: CharOffset,
    direction: MovementDirection,
    unit: MovementUnit,
) -> EngineResult<CharOffset> {
    validate_movement_offset(storage, offset)?;

    match unit {
        MovementUnit::Grapheme => match direction {
            MovementDirection::Previous => storage.previous_grapheme_boundary(offset),
            MovementDirection::Next => storage.next_grapheme_boundary(offset),
        },
        MovementUnit::Word => {
            let text = storage.text();
            let spans = unicode_word_spans(text.as_ref());
            Ok(boundary_from_token_spans(
                &spans,
                offset,
                direction,
                storage.len_chars(),
            ))
        }
        MovementUnit::Identifier => {
            let text = storage.text();
            let spans = identifier_spans(text.as_ref(), policy);
            Ok(boundary_from_token_spans(
                &spans,
                offset,
                direction,
                storage.len_chars(),
            ))
        }
        MovementUnit::Subword => {
            let text = storage.text();
            let spans = subword_spans(text.as_ref());
            Ok(boundary_from_token_spans(
                &spans,
                offset,
                direction,
                storage.len_chars(),
            ))
        }
        MovementUnit::Symbol => {
            let text = storage.text();
            let spans = symbol_spans(text.as_ref(), policy);
            Ok(boundary_from_token_spans(
                &spans,
                offset,
                direction,
                storage.len_chars(),
            ))
        }
    }
}

fn validate_movement_offset<T: TextRead>(storage: &T, offset: CharOffset) -> EngineResult<()> {
    if offset > storage.len_chars() {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    if is_crlf_middle(storage, offset) {
        return Err(EditError::InvalidBoundary { offset }.into());
    }

    if !storage.is_grapheme_boundary(offset)? {
        return Err(CoordinateError::InvalidGraphemeBoundary(offset).into());
    }

    Ok(())
}

fn boundary_from_token_spans(
    spans: &[MovementTokenSpan],
    offset: CharOffset,
    direction: MovementDirection,
    text_len: CharOffset,
) -> CharOffset {
    match direction {
        MovementDirection::Next => spans
            .iter()
            .find_map(|span| {
                if offset < span.start {
                    Some(span.start)
                } else if offset < span.end {
                    Some(span.end)
                } else {
                    None
                }
            })
            .unwrap_or(text_len),
        MovementDirection::Previous => spans
            .iter()
            .rev()
            .find_map(|span| {
                if offset > span.end {
                    Some(span.end)
                } else if offset > span.start {
                    Some(span.start)
                } else {
                    None
                }
            })
            .unwrap_or(CharOffset::ZERO),
    }
}

fn unicode_word_spans(text: &str) -> Vec<MovementTokenSpan> {
    let mut spans: Vec<MovementTokenSpan> = Vec::new();

    for (byte_start, word) in text.unicode_word_indices() {
        let start = text[..byte_start].chars().count();
        let end = start + word.chars().count();

        if let Some(previous) = spans.last_mut() {
            if previous.end.get() == start {
                previous.end = CharOffset::new(end);
                continue;
            }
        }

        spans.push(MovementTokenSpan::new(start, end));
    }

    spans
}

fn identifier_spans(text: &str, policy: WordBoundaryPolicy) -> Vec<MovementTokenSpan> {
    contiguous_spans_by_char(text, |ch| policy.is_identifier_continue(ch))
}

fn symbol_spans(text: &str, policy: WordBoundaryPolicy) -> Vec<MovementTokenSpan> {
    contiguous_spans_by_char(text, |ch| policy.is_symbol_char(ch))
}

fn contiguous_spans_by_char(
    text: &str,
    mut predicate: impl FnMut(char) -> bool,
) -> Vec<MovementTokenSpan> {
    let mut spans = Vec::new();
    let mut current_start = None;

    for (idx, ch) in text.chars().enumerate() {
        if predicate(ch) {
            current_start.get_or_insert(idx);
        } else if let Some(start) = current_start.take() {
            spans.push(MovementTokenSpan::new(start, idx));
        }
    }

    if let Some(start) = current_start {
        spans.push(MovementTokenSpan::new(start, text.chars().count()));
    }

    spans
}

fn subword_spans(text: &str) -> Vec<MovementTokenSpan> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut spans = Vec::new();
    let mut current_start = None;

    for idx in 0..chars.len() {
        let ch = chars[idx];

        if !is_subword_body_char(ch) {
            if let Some(start) = current_start.take() {
                spans.push(MovementTokenSpan::new(start, idx));
            }
            continue;
        }

        match current_start {
            None => current_start = Some(idx),
            Some(start) => {
                let previous = chars[idx - 1];
                let next = chars.get(idx + 1).copied();

                if should_start_new_subword(previous, ch, next) {
                    spans.push(MovementTokenSpan::new(start, idx));
                    current_start = Some(idx);
                }
            }
        }
    }

    if let Some(start) = current_start {
        spans.push(MovementTokenSpan::new(start, chars.len()));
    }

    spans
}

fn is_subword_body_char(ch: char) -> bool {
    ch.is_alphanumeric() || is_combining_mark_for_movement(ch)
}

fn should_start_new_subword(previous: char, current: char, next: Option<char>) -> bool {
    if is_combining_mark_for_movement(current) || is_combining_mark_for_movement(previous) {
        return false;
    }

    (previous.is_lowercase() && current.is_uppercase())
        || (previous.is_alphabetic() && current.is_numeric())
        || (previous.is_numeric() && current.is_alphabetic())
        || (previous.is_uppercase()
            && current.is_uppercase()
            && next.is_some_and(char::is_lowercase))
}

fn is_combining_mark_for_movement(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F
    )
}
