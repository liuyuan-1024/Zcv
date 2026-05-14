//! 文本移动语义：实现 grapheme、word、identifier、subword、symbol 等纯文本边界查找。
//!
//! 本文件只移动 selection/head 并尊重 BufferConfig 策略，不绑定快捷键，也不承担 UI 渲染或命令层语义。

use crate::{
    CharOffset, CoordinateError, EditError, EngineResult, MovementDirection, MovementUnit,
    Selection, SelectionSet, WordBoundaryPolicy, storage::TextRead,
};

use super::{Buffer, coordinates::is_crlf_middle};

impl Buffer {
    /// 按给定移动粒度寻找前一个边界。
    pub fn previous_movement_boundary(
        &self,
        offset: CharOffset,
        unit: MovementUnit,
    ) -> EngineResult<CharOffset> {
        self.movement_boundary(offset, MovementDirection::Previous, unit)
    }

    /// 按给定移动粒度寻找后一个边界。
    pub fn next_movement_boundary(
        &self,
        offset: CharOffset,
        unit: MovementUnit,
    ) -> EngineResult<CharOffset> {
        self.movement_boundary(offset, MovementDirection::Next, unit)
    }

    /// 统一移动边界入口。
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

    /// 移动一组选区的 head。
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
                // Selection.head 是 ByteOffset 深核坐标；movement 边界按 grapheme/char 投影扫描。
                let head_char = self.storage.byte_to_char(selection.head())?;
                let new_head_char = self.movement_boundary(head_char, direction, unit)?;
                let new_head = self.storage.char_to_byte(new_head_char)?;

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

    /// 移动当前 Buffer selection 的便捷入口。
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
struct MovementGrapheme {
    start: CharOffset,
    end: CharOffset,
    first: char,
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
            MovementDirection::Previous => storage.previous_grapheme_boundary_char(offset),
            MovementDirection::Next => storage.next_grapheme_boundary_char(offset),
        },
        MovementUnit::Word => {
            contiguous_grapheme_boundary(storage, offset, direction, is_word_char)
        }
        MovementUnit::Identifier => {
            contiguous_grapheme_boundary(storage, offset, direction, |ch| {
                policy.is_identifier_continue(ch)
            })
        }
        MovementUnit::Subword => subword_boundary(storage, offset, direction),
        MovementUnit::Symbol => {
            contiguous_grapheme_boundary(storage, offset, direction, |ch| policy.is_symbol_char(ch))
        }
    }
}

fn validate_movement_offset<T: TextRead>(storage: &T, offset: CharOffset) -> EngineResult<()> {
    if offset > storage.len_chars() {
        return Err(CoordinateError::CharOutOfBounds(offset).into());
    }

    if is_crlf_middle(storage, offset) {
        let byte = storage.char_to_byte(offset)?;
        return Err(EditError::InvalidBoundary { offset: byte }.into());
    }

    if !storage.is_grapheme_boundary_char(offset)? {
        let byte = storage.char_to_byte(offset)?;
        return Err(CoordinateError::InvalidGraphemeBoundary(byte).into());
    }

    Ok(())
}

fn contiguous_grapheme_boundary<T: TextRead>(
    storage: &T,
    offset: CharOffset,
    direction: MovementDirection,
    mut is_body: impl FnMut(char) -> bool,
) -> EngineResult<CharOffset> {
    match direction {
        MovementDirection::Next => {
            let mut cursor = offset;
            let mut skipped_separator = false;

            while let Some(grapheme) = grapheme_at(storage, cursor)? {
                if is_body(grapheme.first) {
                    if skipped_separator {
                        return Ok(grapheme.start);
                    }
                    return scan_contiguous_end(storage, grapheme, &mut is_body);
                }

                skipped_separator = true;
                cursor = grapheme.end;
            }

            Ok(storage.len_chars())
        }
        MovementDirection::Previous => {
            let mut cursor = offset;
            let mut skipped_separator = false;

            while let Some(grapheme) = grapheme_before(storage, cursor)? {
                if is_body(grapheme.first) {
                    if skipped_separator {
                        return Ok(grapheme.end);
                    }
                    return scan_contiguous_start(storage, grapheme, &mut is_body);
                }

                skipped_separator = true;
                cursor = grapheme.start;
            }

            Ok(CharOffset::ZERO)
        }
    }
}

fn scan_contiguous_end<T: TextRead>(
    storage: &T,
    mut current: MovementGrapheme,
    is_body: &mut impl FnMut(char) -> bool,
) -> EngineResult<CharOffset> {
    loop {
        match grapheme_at(storage, current.end)? {
            Some(next) if is_body(next.first) => current = next,
            _ => return Ok(current.end),
        }
    }
}

fn scan_contiguous_start<T: TextRead>(
    storage: &T,
    mut current: MovementGrapheme,
    is_body: &mut impl FnMut(char) -> bool,
) -> EngineResult<CharOffset> {
    loop {
        match grapheme_before(storage, current.start)? {
            Some(previous) if is_body(previous.first) => current = previous,
            _ => return Ok(current.start),
        }
    }
}

fn subword_boundary<T: TextRead>(
    storage: &T,
    offset: CharOffset,
    direction: MovementDirection,
) -> EngineResult<CharOffset> {
    match direction {
        MovementDirection::Next => next_subword_boundary(storage, offset),
        MovementDirection::Previous => previous_subword_boundary(storage, offset),
    }
}

fn next_subword_boundary<T: TextRead>(storage: &T, offset: CharOffset) -> EngineResult<CharOffset> {
    let mut cursor = offset;

    while let Some(grapheme) = grapheme_at(storage, cursor)? {
        if !is_subword_body_char(grapheme.first) {
            cursor = grapheme.end;
            continue;
        }

        if grapheme.start > offset {
            return Ok(grapheme.start);
        }

        return scan_subword_end(storage, grapheme);
    }

    Ok(storage.len_chars())
}

fn previous_subword_boundary<T: TextRead>(
    storage: &T,
    offset: CharOffset,
) -> EngineResult<CharOffset> {
    let mut cursor = offset;
    let mut skipped_separator = false;

    while let Some(grapheme) = grapheme_before(storage, cursor)? {
        if !is_subword_body_char(grapheme.first) {
            skipped_separator = true;
            cursor = grapheme.start;
            continue;
        }

        if skipped_separator {
            return Ok(grapheme.end);
        }

        return scan_subword_start(storage, grapheme);
    }

    Ok(CharOffset::ZERO)
}

fn scan_subword_end<T: TextRead>(
    storage: &T,
    mut current: MovementGrapheme,
) -> EngineResult<CharOffset> {
    loop {
        let Some(next) = grapheme_at(storage, current.end)? else {
            return Ok(current.end);
        };

        if !is_subword_body_char(next.first) {
            return Ok(current.end);
        }

        let after_next = grapheme_at(storage, next.end)?.map(|grapheme| grapheme.first);
        if should_start_new_subword(current.first, next.first, after_next) {
            return Ok(next.start);
        }

        current = next;
    }
}

fn scan_subword_start<T: TextRead>(
    storage: &T,
    mut current: MovementGrapheme,
) -> EngineResult<CharOffset> {
    loop {
        let Some(previous) = grapheme_before(storage, current.start)? else {
            return Ok(current.start);
        };

        if !is_subword_body_char(previous.first) {
            return Ok(current.start);
        }

        let next = grapheme_at(storage, current.end)?.map(|grapheme| grapheme.first);
        if should_start_new_subword(previous.first, current.first, next) {
            return Ok(current.start);
        }

        current = previous;
    }
}

fn grapheme_at<T: TextRead>(
    storage: &T,
    start: CharOffset,
) -> EngineResult<Option<MovementGrapheme>> {
    if start >= storage.len_chars() {
        return Ok(None);
    }

    let end = storage.next_grapheme_boundary_char(start)?;
    let Some(first) = storage.char_at(start) else {
        return Err(CoordinateError::CharOutOfBounds(start).into());
    };

    Ok(Some(MovementGrapheme { start, end, first }))
}

fn grapheme_before<T: TextRead>(
    storage: &T,
    end: CharOffset,
) -> EngineResult<Option<MovementGrapheme>> {
    if end == CharOffset::ZERO {
        return Ok(None);
    }

    let start = storage.previous_grapheme_boundary_char(end)?;
    let Some(first) = storage.char_at(start) else {
        return Err(CoordinateError::CharOutOfBounds(start).into());
    };

    Ok(Some(MovementGrapheme { start, end, first }))
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || crate::config::is_combining_mark(ch)
}

fn is_subword_body_char(ch: char) -> bool {
    // grapheme cluster 的首字符；其后的组合标记由 grapheme cluster 自动收纳。
    // 这里只判断"是否是单词体字符"。
    ch.is_alphanumeric() || crate::config::is_combining_mark(ch)
}

fn should_start_new_subword(previous: char, current: char, next: Option<char>) -> bool {
    // 组合标记不应触发 camelCase / 数字-字母切分（grapheme cluster 内部不会传到这里，
    // 但跨 cluster 边界若任一端是组合标记，仍应抑制切分）。
    if crate::config::is_combining_mark(current) || crate::config::is_combining_mark(previous) {
        return false;
    }

    (previous.is_lowercase() && current.is_uppercase())
        || (previous.is_alphabetic() && current.is_numeric())
        || (previous.is_numeric() && current.is_alphabetic())
        || (previous.is_uppercase()
            && current.is_uppercase()
            && next.is_some_and(char::is_lowercase))
}
