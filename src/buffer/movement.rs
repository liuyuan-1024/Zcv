//! 文本移动语义：实现 grapheme、word、identifier、subword、symbol 等纯文本边界查找。
//!
//! 本文件只移动 selection/head 并尊重 BufferConfig 策略，不绑定快捷键，也不承担 UI 渲染或命令层语义。

use unicode_segmentation::UnicodeSegmentation;

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
                // Selection.head 是 ByteOffset 深核坐标；token-span 算法当前仍在 char 空间，
                // 因此在边界转 char、计算后再换回 byte。
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
            MovementDirection::Previous => storage.previous_grapheme_boundary_char(offset),
            MovementDirection::Next => storage.next_grapheme_boundary_char(offset),
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
    contiguous_spans_by_grapheme(text, |ch| policy.is_identifier_continue(ch))
}

fn symbol_spans(text: &str, policy: WordBoundaryPolicy) -> Vec<MovementTokenSpan> {
    contiguous_spans_by_grapheme(text, |ch| policy.is_symbol_char(ch))
}

/// 按 grapheme cluster 切分 token，predicate 只针对每个 cluster 的**首字符**。
///
/// 这样合成字符（`é`）、ZWJ emoji 序列、国旗 emoji 等多 codepoint cluster 不会在
/// cluster 中间被切开。返回的 span 边界仍按 char 计数（与 `LogicalColumn` / `CharOffset` 对齐）。
fn contiguous_spans_by_grapheme(
    text: &str,
    mut predicate: impl FnMut(char) -> bool,
) -> Vec<MovementTokenSpan> {
    let mut spans = Vec::new();
    let mut current_start: Option<usize> = None;
    let mut char_idx = 0usize;

    for grapheme in text.graphemes(true) {
        let first = grapheme.chars().next().expect("grapheme cluster 非空");
        let cluster_len = grapheme.chars().count();

        if predicate(first) {
            current_start.get_or_insert(char_idx);
        } else if let Some(start) = current_start.take() {
            spans.push(MovementTokenSpan::new(start, char_idx));
        }

        char_idx += cluster_len;
    }

    if let Some(start) = current_start {
        spans.push(MovementTokenSpan::new(start, char_idx));
    }

    spans
}

fn subword_spans(text: &str) -> Vec<MovementTokenSpan> {
    // 收集每个 grapheme 的「首字符 + 起始 char 偏移 + cluster 长度」。
    // subword 分割逻辑只关注 cluster 之间的边界，cluster 内的组合标记 / ZWJ / RI 都不参与。
    let clusters: Vec<(usize, char, usize)> = {
        let mut out = Vec::new();
        let mut char_idx = 0usize;
        for grapheme in text.graphemes(true) {
            let first = grapheme.chars().next().expect("grapheme cluster 非空");
            let len = grapheme.chars().count();
            out.push((char_idx, first, len));
            char_idx += len;
        }
        out
    };

    let mut spans = Vec::new();
    let mut current_start: Option<usize> = None;

    for i in 0..clusters.len() {
        let (start_idx, ch, len) = clusters[i];

        if !is_subword_body_char(ch) {
            if let Some(start) = current_start.take() {
                spans.push(MovementTokenSpan::new(start, start_idx));
            }
            continue;
        }

        match current_start {
            None => current_start = Some(start_idx),
            Some(start) => {
                let previous = clusters[i - 1].1;
                let next = clusters.get(i + 1).map(|(_, c, _)| *c);

                if should_start_new_subword(previous, ch, next) {
                    spans.push(MovementTokenSpan::new(start, start_idx));
                    current_start = Some(start_idx);
                }
            }
        }

        let _ = len; // cluster 长度已经在累加 start_idx 时使用
    }

    if let Some(start) = current_start {
        let end_chars = clusters.last().map(|(idx, _, len)| idx + len).unwrap_or(0);
        spans.push(MovementTokenSpan::new(start, end_chars));
    }

    spans
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
