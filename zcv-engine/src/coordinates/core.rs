//! 共享坐标数学核心：为 Buffer 与 Snapshot 提供基于 TextRead 的行内逻辑列、视觉列和 tab 宽度计算。
//!
//! 本文件保持无状态、无存储后端偏好，只依赖只读文本能力与 BufferConfig，不处理文本变异。
//!
//! **Grapheme 正确性纪律**：所有"按可见单位前进"的列宽计算按 **grapheme cluster** 走，
//! 不按 char (Unicode scalar) 走。
//! - 合成字符 `é = e + U+0301` 视为 1 个 grapheme、宽度 1
//! - emoji ZWJ 序列 `👨‍👩‍👧` 视为 1 个 grapheme、宽度 2（首字符宽度）
//! - 国旗 emoji `🇨🇳` 视为 1 个 grapheme、宽度 2
//! - `LogicalColumn` 仍按 char 数推进（与 ropey 的 `len_chars` 对齐），仅 `DisplayColumn` 按 grapheme 计算

use unicode_segmentation::UnicodeSegmentation;

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

    // slice_text 在单块 rope 时返回 Cow::Borrowed（零拷贝），多块时才物化。
    // grapheme cluster 边界要求一段连续文本，无法纯 chunk-streaming。
    let text = storage.slice_text(range)?;

    Ok(DisplayColumn::new(display_width_of_text(
        text.as_ref(),
        config,
    )))
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

    if target == 0 {
        return Ok(LogicalColumn::ZERO);
    }

    let text = storage.slice_text(range)?;
    let mut current_display = 0usize;
    let mut current_logical = 0usize;

    // 按 grapheme cluster 推进 display；logical 按 char 数累加
    for grapheme in text.as_ref().graphemes(true) {
        let next_display = advance_display_column_for_grapheme(current_display, grapheme, config);
        let next_logical = current_logical + grapheme.chars().count();

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

/// 按 grapheme cluster 计算文本的显示宽度。
///
/// 与按 char 累加相比，**合成字符、ZWJ emoji 序列、国旗 emoji** 等多 codepoint 的字素簇
/// 按一个可见单位计算宽度，避免「光标停在 emoji 中间」「合成字符占两列」之类的经典 bug。
fn display_width_of_text(text: &str, config: &BufferConfig) -> usize {
    text.graphemes(true)
        .fold(0usize, |display_column, grapheme| {
            advance_display_column_for_grapheme(display_column, grapheme, config)
        })
}

/// 推进一个 grapheme cluster 的显示列。
///
/// - 制表符按 tab stop 推进
/// - 其他 grapheme：宽度 = **首字符宽度**；同 grapheme 内后续 codepoint
///   （组合标记 / ZWJ / 第二个 RI 等）按 0 宽度处理
fn advance_display_column_for_grapheme(
    display_column: usize,
    grapheme: &str,
    config: &BufferConfig,
) -> usize {
    if grapheme == "\t" {
        return next_tab_stop(DisplayColumn::new(display_column), config.tab.tab_width()).get();
    }

    let Some(first_char) = grapheme.chars().next() else {
        return display_column;
    };

    // grapheme 的宽度由首字符决定；组合标记 / ZWJ / RI 在同一 cluster 内贡献 0
    display_column + config.display_width.char_width(first_char)
}
