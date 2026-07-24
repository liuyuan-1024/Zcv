//! 文本移动语义：实现 grapheme、word、identifier、subword、symbol 等纯文本边界查找。
//!
//! 本文件只移动 selection/head 并尊重 BufferConfig 策略，不绑定快捷键，也不承担 UI 渲染或命令层语义。

use crate::{
    CharOffset, CoordinateError, EditError, EngineResult, Line, Motion, MovementDirection,
    MovementUnit, Selection, SelectionSet, WordBoundaryPolicy, config::WordBoundaryClassifier,
    storage::TextRead,
};

use super::{Buffer, coordinates::is_crlf_middle};

impl Buffer {
    /// 按给定移动粒度寻找前一个边界。
    pub fn previous_movement_boundary(
        &self,
        offset: CharOffset,
        unit: MovementUnit,
    ) -> EngineResult<CharOffset> {
        self.movement_boundary(offset, MovementDirection::Previous, Motion::ByUnit(unit))
    }

    /// 按给定移动粒度寻找后一个边界。
    pub fn next_movement_boundary(
        &self,
        offset: CharOffset,
        unit: MovementUnit,
    ) -> EngineResult<CharOffset> {
        self.movement_boundary(offset, MovementDirection::Next, Motion::ByUnit(unit))
    }

    /// 统一光标运动入口。
    ///
    /// `motion` 接受 `impl Into<Motion>`，所以调用方既可传 `Motion::LineStep`，
    /// 也可直接传任一 `MovementUnit`（自动包装为 `Motion::ByUnit(...)`）。
    pub fn movement_boundary(
        &self,
        offset: CharOffset,
        direction: MovementDirection,
        motion: impl Into<Motion>,
    ) -> EngineResult<CharOffset> {
        match motion.into() {
            Motion::ByUnit(unit) => movement_boundary_in_text(
                &self.storage,
                self.config.word_boundary,
                offset,
                direction,
                unit,
            ),
            // LineStep / PageStep 需要 DisplayColumn / TabConfig 等完整 Buffer 配置，
            // 不属于 storage-only 的 movement_boundary_in_text 契约。
            Motion::LineStep => self.line_step_target(offset, direction, 1),
            Motion::PageStep { lines } => {
                self.line_step_target(offset, direction, lines.max(1) as usize)
            }
        }
    }

    /// 上下移动 `step` 行的列位投影，供 LineStep / PageStep 共用。
    ///
    /// 边界规则与 LineStep 对称：当前已经在首行再向上 → 文档开头；当前已经在末行再向下 → 文档末尾。
    /// 否则按 `step` 截断到 `[0, line_count − 1]` 范围内，落到目标行同 display column。
    ///
    /// v1 无 sticky column —— 列位每次都用当前 caret 的 display column 重新取，跨长短行时可能"卡列"。
    /// 完整体验需要把 sticky column 加到 selection 状态或外部维护，留作后续迭代。
    fn line_step_target(
        &self,
        offset: CharOffset,
        direction: MovementDirection,
        step: usize,
    ) -> EngineResult<CharOffset> {
        let byte = self.storage.char_to_byte(offset)?;
        let current_line = self.storage.byte_to_position(byte)?.line().get();
        let line_count = self.storage.line_count();
        let last_line = line_count.saturating_sub(1);

        let target_line = match direction {
            MovementDirection::Previous => {
                if current_line == 0 {
                    // 已在首行：再向上 → 文档开头。
                    return Ok(CharOffset::ZERO);
                }
                Line::new(current_line.saturating_sub(step))
            }
            MovementDirection::Next => {
                if current_line >= last_line {
                    // 已在末行：再向下 → 文档末尾。
                    return self.storage.byte_to_char(self.storage.len_bytes());
                }
                Line::new(current_line.saturating_add(step).min(last_line))
            }
        };

        let target_col = self.char_to_display_column(offset)?;
        self.display_column_to_char(target_line, target_col)
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
    /// `motion` 接 `impl Into<Motion>`：传 `Motion::LineStep` 走垂直；传任一 `MovementUnit`
    /// 自动包装为 `Motion::ByUnit(...)` 走粒度边界。`extend = false` 时移动后塌缩为 caret；
    /// `extend = true` 时保留 anchor，扩展/收缩选区。
    /// 该 API 是纯计算：接收 SelectionSet 并返回移动结果，不提交文本事务。
    pub fn move_selections(
        &self,
        selections: &SelectionSet,
        direction: MovementDirection,
        motion: impl Into<Motion>,
        extend: bool,
    ) -> EngineResult<SelectionSet> {
        self.validate_selection_set(selections)?;
        let motion = motion.into();

        let primary_index = selections.primary_index();
        let moved = selections
            .as_slice()
            .iter()
            .copied()
            .map(|selection| {
                // Selection.head 是 ByteOffset 深核坐标；movement 边界按 grapheme/char 投影扫描。
                let head_char = self.storage.byte_to_char(selection.head())?;
                let new_head_char = self.movement_boundary(head_char, direction, motion)?;
                let new_head = self.storage.char_to_byte(new_head_char)?;

                Ok(if extend {
                    selection.with_head(new_head)
                } else {
                    Selection::caret(new_head)
                })
            })
            .collect::<EngineResult<Vec<_>>>()?;

        let moved = SelectionSet::new_with_primary(moved, primary_index);
        Ok(moved)
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
) -> EngineResult<CharOffset> {
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

fn word_boundary<T: TextRead>(
    storage: &T,
    offset: CharOffset,
    direction: MovementDirection,
    policy: WordBoundaryPolicy,
    unit: MovementUnit,
) -> EngineResult<CharOffset> {
    let Some(classifier) = policy.classifier(unit) else {
        return movement_unit_bug(unit);
    };

    if classifier.is_subword() {
        subword_boundary(storage, offset, direction, classifier)
    } else {
        word_like_boundary(storage, offset, direction, classifier)
    }
}

fn movement_unit_bug(unit: MovementUnit) -> EngineResult<CharOffset> {
    Err(crate::EngineError::EngineBug {
        location: "word_boundary",
        detail: format!("非词类移动粒度不应进入词边界策略: {unit:?}"),
    })
}

fn word_like_boundary<T: TextRead>(
    storage: &T,
    offset: CharOffset,
    direction: MovementDirection,
    classifier: WordBoundaryClassifier,
) -> EngineResult<CharOffset> {
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
) -> EngineResult<CharOffset> {
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
) -> EngineResult<CharOffset> {
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
) -> EngineResult<CharOffset> {
    match direction {
        MovementDirection::Next => next_subword_boundary(storage, offset, classifier),
        MovementDirection::Previous => previous_subword_boundary(storage, offset, classifier),
    }
}

fn next_subword_boundary<T: TextRead>(
    storage: &T,
    offset: CharOffset,
    classifier: WordBoundaryClassifier,
) -> EngineResult<CharOffset> {
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
) -> EngineResult<CharOffset> {
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
) -> EngineResult<CharOffset> {
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
) -> EngineResult<CharOffset> {
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
