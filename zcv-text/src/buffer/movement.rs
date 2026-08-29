//! 文本移动语义：实现 grapheme、word、identifier、subword、symbol 等纯文本边界查找。
//!
//! 本文件只移动 selection/head 并尊重 BufferConfig 策略，不绑定快捷键，也不承担 UI 渲染或命令层语义。

use super::{Buffer, coordinates::is_crlf_middle};
use crate::{
    config::{WordBoundaryClassifier, WordBoundaryPolicy},
    errors::{CoordinateError, EditError, TextError, TextResult},
    movement::{MovementDirection, MovementUnit},
    storage::TextRead,
    types::{CharOffset, Line},
};

impl Buffer {
    /// 按纯文本粒度查找相邻边界。垂直移动与 selection 变换由宿主 Editor 负责。
    pub fn movement_boundary(
        &self,
        offset: CharOffset,
        direction: MovementDirection,
        unit: MovementUnit,
    ) -> TextResult<CharOffset> {
        movement_boundary_in_text(
            &self.storage,
            self.config.word_boundary,
            offset,
            direction,
            unit,
        )
    }

    /// 以 offset 为中心取连续同类字符范围（双击选词语义）。
    ///
    /// 语义对齐 Zed 的 `surrounding_word`：目标类别取光标前后字符中"更词"的那一个（Word > Symbol > Space），随后向左右扫描吃掉连续同类字符；
    /// 换行不参与任何类别的连续性。
    /// 扫描按 grapheme 边界推进，零宽字符（组合音标等）随前导字符归属同一词。
    pub fn surrounding_word(&self, offset: CharOffset) -> TextResult<(CharOffset, CharOffset)> {
        surrounding_word_in_text(&self.storage, self.config.word_boundary, offset)
    }

    /// 光标前后都是词字符时返回 true（对齐 Zed 的 `is_inside_word`）。
    ///
    /// 拖拽扩展选区时用于判断光标是否仍停留在某个词内部，决定是否按整词边界吸附。
    pub fn is_inside_word(&self, offset: CharOffset) -> TextResult<bool> {
        let Some(classifier) = self
            .config
            .word_boundary
            .classifier(MovementUnit::Identifier)
        else {
            return movement_unit_bug(MovementUnit::Identifier);
        };
        let previous = grapheme_before(&self.storage, offset)?
            .is_some_and(|grapheme| classifier.is_body(grapheme.first));
        let next = grapheme_at(&self.storage, offset)?
            .is_some_and(|grapheme| classifier.is_body(grapheme.first));
        Ok(previous && next)
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
) -> TextResult<CharOffset> {
    validate_movement_offset(storage, offset)?;

    match unit {
        MovementUnit::Grapheme => match direction {
            MovementDirection::Previous => storage.previous_grapheme_boundary_char(offset),
            MovementDirection::Next => storage.next_grapheme_boundary_char(offset),
        },
        MovementUnit::Word
        | MovementUnit::Identifier
        | MovementUnit::Subword
        | MovementUnit::Symbol => word_boundary(storage, offset, direction, policy, unit),
        MovementUnit::LineEdge => line_edge_boundary(storage, offset, direction),
    }
}

fn line_edge_boundary<T: TextRead>(
    storage: &T,
    offset: CharOffset,
    direction: MovementDirection,
) -> TextResult<CharOffset> {
    let byte = storage.char_to_byte(offset)?;
    let position = storage.byte_to_position(byte)?;
    let line = position.line();

    match direction {
        MovementDirection::Previous => {
            let line_start_byte = storage.line_start(line)?;
            storage.byte_to_char(line_start_byte)
        }
        MovementDirection::Next => {
            let line_count = storage.line_count();
            let next_line = line.get().saturating_add(1);
            if next_line >= line_count {
                // 末行：行尾即文档末尾。
                storage.byte_to_char(storage.len_bytes())
            } else {
                // 跳到下一行行首，再回退一个 grapheme 即可越过 \n 或 \r\n。
                let next_start = storage.line_start(Line::new(next_line))?;
                let end_byte = storage.previous_grapheme_boundary(next_start)?;
                storage.byte_to_char(end_byte)
            }
        }
    }
}

fn validate_movement_offset<T: TextRead>(storage: &T, offset: CharOffset) -> TextResult<()> {
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

fn word_boundary<T: TextRead>(
    storage: &T,
    offset: CharOffset,
    direction: MovementDirection,
    policy: WordBoundaryPolicy,
    unit: MovementUnit,
) -> TextResult<CharOffset> {
    let Some(classifier) = policy.classifier(unit) else {
        return movement_unit_bug(unit);
    };

    if classifier.is_subword() {
        subword_boundary(storage, offset, direction, classifier)
    } else {
        word_like_boundary(storage, offset, direction, classifier)
    }
}

fn movement_unit_bug<T>(unit: MovementUnit) -> TextResult<T> {
    Err(TextError::InvariantViolation {
        location: "word_boundary",
        detail: format!("非词类移动粒度不应进入词边界策略: {unit:?}"),
    })
}

fn word_like_boundary<T: TextRead>(
    storage: &T,
    offset: CharOffset,
    direction: MovementDirection,
    classifier: WordBoundaryClassifier,
) -> TextResult<CharOffset> {
    match direction {
        MovementDirection::Next => {
            let mut cursor = offset;
            let mut skipped_separator = false;
            let mut sep_kind: Option<SeparatorKind> = None;

            while let Some(grapheme) = grapheme_at(storage, cursor)? {
                if classifier.is_body(grapheme.first) {
                    if skipped_separator {
                        return Ok(grapheme.start);
                    }
                    return scan_contiguous_end(storage, grapheme, classifier);
                }

                let kind = separator_kind(grapheme.first);

                // 换行不分连续：每个 \n 独立为一个删除单元。
                if kind == SeparatorKind::Newline && skipped_separator {
                    return Ok(cursor);
                }

                // 分隔符类别切换 → 停止，不同类别不混合删除。
                if let Some(prev) = sep_kind {
                    if prev != kind {
                        return Ok(cursor);
                    }
                } else {
                    sep_kind = Some(kind);
                }

                skipped_separator = true;
                cursor = grapheme.end;
            }

            Ok(storage.len_chars())
        }
        MovementDirection::Previous => {
            let mut cursor = offset;
            let mut skipped_separator = false;
            let mut sep_kind: Option<SeparatorKind> = None;

            while let Some(grapheme) = grapheme_before(storage, cursor)? {
                if classifier.is_body(grapheme.first) {
                    if skipped_separator {
                        return Ok(grapheme.end);
                    }
                    return scan_contiguous_start(storage, grapheme, classifier);
                }

                let kind = separator_kind(grapheme.first);

                // 换行不分连续：每个 \n 独立为一个删除单元。
                if kind == SeparatorKind::Newline && skipped_separator {
                    return Ok(cursor);
                }

                // 分隔符类别切换 → 停止。
                if let Some(prev) = sep_kind {
                    if prev != kind {
                        return Ok(cursor);
                    }
                } else {
                    sep_kind = Some(kind);
                }

                skipped_separator = true;
                cursor = grapheme.start;
            }

            Ok(CharOffset::ZERO)
        }
    }
}

/// 分隔符分类：空格（\t 在内）可连续合并；换行独立为单个删除单元；其余符号为第三类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeparatorKind {
    Space,
    Newline,
    Symbol,
}

fn separator_kind(ch: char) -> SeparatorKind {
    match ch {
        ' ' | '\t' => SeparatorKind::Space,
        '\n' | '\r' => SeparatorKind::Newline,
        _ => SeparatorKind::Symbol,
    }
}

fn scan_contiguous_end<T: TextRead>(
    storage: &T,
    mut current: MovementGrapheme,
    classifier: WordBoundaryClassifier,
) -> TextResult<CharOffset> {
    loop {
        match grapheme_at(storage, current.end)? {
            Some(next) if classifier.is_body(next.first) => current = next,
            _ => return Ok(current.end),
        }
    }
}

fn scan_contiguous_start<T: TextRead>(
    storage: &T,
    mut current: MovementGrapheme,
    classifier: WordBoundaryClassifier,
) -> TextResult<CharOffset> {
    loop {
        match grapheme_before(storage, current.start)? {
            Some(previous) if classifier.is_body(previous.first) => current = previous,
            _ => return Ok(current.start),
        }
    }
}

fn subword_boundary<T: TextRead>(
    storage: &T,
    offset: CharOffset,
    direction: MovementDirection,
    classifier: WordBoundaryClassifier,
) -> TextResult<CharOffset> {
    match direction {
        MovementDirection::Next => next_subword_boundary(storage, offset, classifier),
        MovementDirection::Previous => previous_subword_boundary(storage, offset, classifier),
    }
}

fn next_subword_boundary<T: TextRead>(
    storage: &T,
    offset: CharOffset,
    classifier: WordBoundaryClassifier,
) -> TextResult<CharOffset> {
    let mut cursor = offset;
    let mut sep_kind: Option<SeparatorKind> = None;

    while let Some(grapheme) = grapheme_at(storage, cursor)? {
        if !classifier.is_body(grapheme.first) {
            let kind = separator_kind(grapheme.first);

            if kind == SeparatorKind::Newline && sep_kind.is_some() {
                return Ok(cursor);
            }

            if let Some(prev) = sep_kind {
                if prev != kind {
                    return Ok(cursor);
                }
            } else {
                sep_kind = Some(kind);
            }

            cursor = grapheme.end;
            continue;
        }

        if grapheme.start > offset {
            return Ok(grapheme.start);
        }

        return scan_subword_end(storage, grapheme, classifier);
    }

    Ok(storage.len_chars())
}

fn previous_subword_boundary<T: TextRead>(
    storage: &T,
    offset: CharOffset,
    classifier: WordBoundaryClassifier,
) -> TextResult<CharOffset> {
    let mut cursor = offset;
    let mut skipped_separator = false;
    let mut sep_kind: Option<SeparatorKind> = None;

    while let Some(grapheme) = grapheme_before(storage, cursor)? {
        if !classifier.is_body(grapheme.first) {
            let kind = separator_kind(grapheme.first);

            if kind == SeparatorKind::Newline && skipped_separator {
                return Ok(cursor);
            }

            if let Some(prev) = sep_kind {
                if prev != kind {
                    return Ok(cursor);
                }
            } else {
                sep_kind = Some(kind);
            }

            skipped_separator = true;
            cursor = grapheme.start;
            continue;
        }

        if skipped_separator {
            return Ok(grapheme.end);
        }

        return scan_subword_start(storage, grapheme, classifier);
    }

    Ok(CharOffset::ZERO)
}

fn scan_subword_end<T: TextRead>(
    storage: &T,
    mut current: MovementGrapheme,
    classifier: WordBoundaryClassifier,
) -> TextResult<CharOffset> {
    loop {
        let Some(next) = grapheme_at(storage, current.end)? else {
            return Ok(current.end);
        };

        if !classifier.is_body(next.first) {
            return Ok(current.end);
        }

        let after_next = grapheme_at(storage, next.end)?.map(|grapheme| grapheme.first);
        if classifier.should_start_new_subword(current.first, next.first, after_next) {
            return Ok(next.start);
        }

        current = next;
    }
}

fn scan_subword_start<T: TextRead>(
    storage: &T,
    mut current: MovementGrapheme,
    classifier: WordBoundaryClassifier,
) -> TextResult<CharOffset> {
    loop {
        let Some(previous) = grapheme_before(storage, current.start)? else {
            return Ok(current.start);
        };

        if !classifier.is_body(previous.first) {
            return Ok(current.start);
        }

        let next = grapheme_at(storage, current.end)?.map(|grapheme| grapheme.first);
        if classifier.should_start_new_subword(previous.first, current.first, next) {
            return Ok(current.start);
        }

        current = previous;
    }
}

fn grapheme_at<T: TextRead>(
    storage: &T,
    start: CharOffset,
) -> TextResult<Option<MovementGrapheme>> {
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
) -> TextResult<Option<MovementGrapheme>> {
    if end == CharOffset::ZERO {
        return Ok(None);
    }

    let start = storage.previous_grapheme_boundary_char(end)?;
    let Some(first) = storage.char_at(start) else {
        return Err(CoordinateError::CharOutOfBounds(start).into());
    };

    Ok(Some(MovementGrapheme { start, end, first }))
}

/// 双击选词的字符三态类别，排序对齐 Zed 的 CharKind（Whitespace < Punctuation < Word）。
/// 换行单独一档且永不作目标，保证词/符号/空格的连续不会跨行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SurroundingKind {
    Newline,
    Space,
    Symbol,
    Word,
}

fn surrounding_kind(classifier: WordBoundaryClassifier, ch: char) -> SurroundingKind {
    if classifier.is_body(ch) {
        SurroundingKind::Word
    } else {
        match separator_kind(ch) {
            SeparatorKind::Space => SurroundingKind::Space,
            SeparatorKind::Newline => SurroundingKind::Newline,
            SeparatorKind::Symbol => SurroundingKind::Symbol,
        }
    }
}

fn surrounding_word_in_text<T: TextRead>(
    storage: &T,
    policy: WordBoundaryPolicy,
    offset: CharOffset,
) -> TextResult<(CharOffset, CharOffset)> {
    validate_movement_offset(storage, offset)?;
    let Some(classifier) = policy.classifier(MovementUnit::Identifier) else {
        return movement_unit_bug(MovementUnit::Identifier);
    };

    let previous = grapheme_before(storage, offset)?;
    let next = grapheme_at(storage, offset)?;
    // 目标类别：光标两侧"更词"的一侧；两侧都为空（空文档）时范围退化到 offset 本身。
    let target = match (previous, next) {
        (Some(prev), Some(next)) => {
            surrounding_kind(classifier, prev.first).max(surrounding_kind(classifier, next.first))
        }
        (Some(prev), None) => surrounding_kind(classifier, prev.first),
        (None, Some(next)) => surrounding_kind(classifier, next.first),
        (None, None) => return Ok((offset, offset)),
    };
    // 光标夹在换行之间时无词可选，保持空范围。
    if target == SurroundingKind::Newline {
        return Ok((offset, offset));
    }

    let mut start = offset;
    while let Some(grapheme) = grapheme_before(storage, start)? {
        if surrounding_kind(classifier, grapheme.first) == target {
            start = grapheme.start;
        } else {
            break;
        }
    }

    let mut end = offset;
    while let Some(grapheme) = grapheme_at(storage, end)? {
        if surrounding_kind(classifier, grapheme.first) == target {
            end = grapheme.end;
        } else {
            break;
        }
    }

    Ok((start, end))
}
