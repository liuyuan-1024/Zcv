//! 共享坐标数学核心：为 Buffer 与 Snapshot 提供基于 TextRead 的行内逻辑列、视觉列和 tab 宽度计算。
//!
//! 本文件保持无状态、无存储后端偏好，只依赖只读文本能力与 BufferConfig，不处理文本变异。
//!
//! **Zero-copy 纪律**：所有列宽计算消费 `TextRead::chunks(range)` 流式迭代器；
//! 不再走 `slice_text → String` 的全量物化路径。

use crate::{
    BufferConfig, CharOffset, DisplayColumn, DisplayColumnAffinity, EngineResult, Line,
    LogicalColumn, Position, TextRange, storage::TextRead,
};

pub(crate) fn char_to_display_column_in_text<T: TextRead>(
    storage: &T,
    config: &BufferConfig,
    offset: CharOffset,
) -> EngineResult<DisplayColumn> {
    let position = storage.char_to_position(offset)?;
    logical_to_display_column_in_text(storage, config, position.line(), position.column())
}

pub(crate) fn logical_to_display_column_in_text<T: TextRead>(
    storage: &T,
    config: &BufferConfig,
    line: Line,
    column: LogicalColumn,
) -> EngineResult<DisplayColumn> {
    let line_start = storage.line_start(line)?;
    let offset = storage.position_to_byte(Position::new(line, column))?;
    let range = TextRange::new(line_start, offset)?;

    // 流式消费 chunks，避免 slice_text 全量物化
    let mut display_column = 0usize;
    for chunk in storage.chunks(range)? {
        for ch in chunk.chars() {
            display_column = advance_display_column(display_column, ch, config);
        }
    }

    Ok(DisplayColumn::new(display_column))
}

pub(crate) fn display_to_logical_column_in_text<T: TextRead>(
    storage: &T,
    config: &BufferConfig,
    line: Line,
    column: DisplayColumn,
    affinity: DisplayColumnAffinity,
) -> EngineResult<LogicalColumn> {
    let line_start = storage.line_start(line)?;
    let line_end = line_content_end_for_storage(storage, line)?;
    let range = TextRange::new(line_start, line_end)?;
    let target = column.get();
    let mut current_display = 0usize;
    let mut current_logical = 0usize;

    if target == 0 {
        return Ok(LogicalColumn::ZERO);
    }

    // 流式消费 chunks
    for chunk in storage.chunks(range)? {
        for ch in chunk.chars() {
            let next_display = advance_display_column(current_display, ch, config);
            let next_logical = current_logical + 1;

            if target == current_display {
                return Ok(LogicalColumn::new(current_logical));
            }

            if target == next_display {
                return Ok(LogicalColumn::new(next_logical));
            }

            if target > current_display && target < next_display {
                return Ok(LogicalColumn::new(match affinity {
                    DisplayColumnAffinity::Previous => current_logical,
                    DisplayColumnAffinity::Next => next_logical,
                    DisplayColumnAffinity::Nearest => {
                        let distance_to_previous = target - current_display;
                        let distance_to_next = next_display - target;

                        if distance_to_previous <= distance_to_next {
                            current_logical
                        } else {
                            next_logical
                        }
                    }
                }));
            }

            current_display = next_display;
            current_logical = next_logical;
        }
    }

    Ok(LogicalColumn::new(current_logical))
}

pub(crate) fn next_tab_stop(display_column: DisplayColumn, tab_width: usize) -> DisplayColumn {
    let current = display_column.get();
    let remainder = current % tab_width;
    let delta = if remainder == 0 {
        tab_width
    } else {
        tab_width - remainder
    };

    DisplayColumn::new(current + delta)
}

fn line_content_end_for_storage<T: TextRead>(
    storage: &T,
    line: Line,
) -> EngineResult<crate::ByteOffset> {
    let line_start = storage.line_start(line)?.get();
    let mut next_line_start = if line.get() + 1 < storage.line_count() {
        storage.line_start(Line::new(line.get() + 1))?.get()
    } else {
        storage.len_bytes().get()
    };

    // 用 byte 接口检测 \n / \r\n
    if next_line_start > line_start
        && storage.char_at_byte(crate::ByteOffset::new(next_line_start - 1)) == Some('\n')
    {
        next_line_start -= 1;

        if next_line_start > line_start
            && storage.char_at_byte(crate::ByteOffset::new(next_line_start - 1)) == Some('\r')
        {
            next_line_start -= 1;
        }
    }

    Ok(crate::ByteOffset::new(next_line_start))
}

fn advance_display_column(display_column: usize, ch: char, config: &BufferConfig) -> usize {
    if ch == '\t' {
        next_tab_stop(DisplayColumn::new(display_column), config.tab.tab_width()).get()
    } else {
        display_column + config.display_width.char_width(ch)
    }
}
