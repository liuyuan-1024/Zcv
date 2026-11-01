//! `MultiBufferSnapshot` 的缩进层级与缩进引导线查询。
//!
//! 本文件的类型与字段仍定义在 crate 根模块 `multi_buffer.rs`。

use super::*;

impl MultiBufferSnapshot {
    pub fn line_indent_for_row(&self, row: Line) -> LineIndent {
        self.line_cursor(row)
            .and_then(|cursor| cursor.line_indent())
            .unwrap_or(LineIndent {
                line_blank: true,
                ..LineIndent::default()
            })
    }

    /// 返回光标所在缩进块的外层边界和外层缩进，供引导线高亮匹配。
    pub fn enclosing_indent(&self, mut target_row: Line) -> Option<(Range<Line>, LineIndent)> {
        let max_row = Line::new(self.line_count().saturating_sub(1));
        if target_row >= max_row {
            return None;
        }

        let mut target_indent = self.line_indent_for_row(target_row);
        if !target_indent.is_line_empty() {
            let next_indent = self.line_indent_for_row(Line::new(target_row.get() + 1));
            if !next_indent.is_line_empty() && target_indent.raw_len() < next_indent.raw_len() {
                target_indent = next_indent;
                target_row = Line::new(target_row.get() + 1);
            }
        }

        const SEARCH_ROW_LIMIT: usize = 25_000;
        const SEARCH_WHITESPACE_ROW_LIMIT: usize = 2_500;
        if target_indent.is_line_empty() {
            let start = target_row.get().saturating_sub(SEARCH_WHITESPACE_ROW_LIMIT);
            let end = target_row
                .get()
                .saturating_add(SEARCH_WHITESPACE_ROW_LIMIT)
                .min(max_row.get());
            let above = (start..=target_row.get()).rev().find_map(|row| {
                let indent = self.line_indent_for_row(Line::new(row));
                (!indent.is_line_empty()).then_some((Line::new(row), indent))
            });
            let below = (target_row.get()..=end).find_map(|row| {
                let indent = self.line_indent_for_row(Line::new(row));
                (!indent.is_line_empty()).then_some((Line::new(row), indent))
            });
            let (row, indent) = match (above, below) {
                (Some(above), Some(below)) => {
                    if above.1.raw_len() >= below.1.raw_len() {
                        above
                    } else {
                        below
                    }
                }
                (Some(above), None) => above,
                (None, Some(below)) => below,
                (None, None) => return None,
            };
            target_row = row;
            target_indent = indent;
        }

        let start = target_row.get().saturating_sub(SEARCH_ROW_LIMIT);
        let end = target_row
            .get()
            .saturating_add(SEARCH_ROW_LIMIT)
            .min(max_row.get() + 1);
        let (start_row, start_indent) = (start..=target_row.get()).rev().find_map(|row| {
            let indent = self.line_indent_for_row(Line::new(row));
            (!indent.is_line_empty() && indent.raw_len() < target_indent.raw_len())
                .then_some((Line::new(row), indent))
        })?;
        let end_indent = (target_row.get()..=end).find_map(|row| {
            let indent = self.line_indent_for_row(Line::new(row));
            (!indent.is_line_empty() && indent.raw_len() < target_indent.raw_len())
                .then_some((Line::new(row.saturating_sub(1)), indent))
        });
        let (end_row, outer_indent) = match end_indent {
            Some((row, indent)) if indent.raw_len() > start_indent.raw_len() => (row, indent),
            Some((row, _)) => (row, start_indent),
            None => (Line::new(end), start_indent),
        };
        Some((start_row..end_row, outer_indent))
    }

    /// 从组合文本快照按需派生可见范围的缩进线段；不保存第二份缩进状态。
    pub fn indent_guides_in_range(
        &self,
        range: Range<Line>,
        ignore_disabled_for_language: bool,
    ) -> Vec<IndentGuide> {
        let Some(mut cursor) = self.line_cursor(range.start) else {
            return Vec::new();
        };
        const TRAILING_ROW_SEARCH_LIMIT: usize = 25;
        let search_end = range
            .end
            .get()
            .saturating_add(TRAILING_ROW_SEARCH_LIMIT)
            .min(self.line_count());
        let mut lines = Vec::new();
        for row in range.start.get()..search_end {
            if !cursor.seek(Line::new(row)) {
                break;
            }
            let Some(excerpt) = cursor.excerpt_snapshot() else {
                continue;
            };
            let Some(source) = cursor.source.as_ref() else {
                continue;
            };
            let settings = source.settings.indent_guides;
            if !settings.enabled && !ignore_disabled_for_language {
                continue;
            }
            let Some(indent) = cursor.line_indent() else {
                continue;
            };
            lines.push(IndentLine {
                row: Line::new(row),
                indent,
                buffer_id: excerpt.buffer_id(),
                tab_size: source.settings.tab.tab_size.get(),
                settings,
            });
        }

        let mut guides = Vec::new();
        let mut stack: Vec<IndentGuide> = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            let first = &lines[index];
            if first.row >= range.end {
                break;
            }
            index += 1;
            let mut line_indent = first.indent;
            let mut last_row = first.row;
            let mut found_indent = !line_indent.is_line_blank();
            if !found_indent {
                while let Some(next) = lines.get(index) {
                    index += 1;
                    if next.indent.is_line_blank() {
                        continue;
                    }
                    line_indent = next.indent;
                    last_row = Line::new(next.row.get().min(range.end.get().saturating_sub(1)));
                    found_indent = true;
                    break;
                }
            }
            let depth = if found_indent {
                line_indent.len(first.tab_size) / first.tab_size
            } else {
                0
            };
            let current_depth = stack.len();
            if depth < current_depth {
                for _ in depth..current_depth {
                    let mut guide = stack.pop().expect("缩进栈深度必须一致");
                    if last_row != first.row {
                        guide.end_row = Line::new(first.row.get().saturating_sub(1));
                    }
                    guides.push(guide);
                }
            } else if depth > current_depth {
                for next_depth in current_depth..depth {
                    stack.push(IndentGuide {
                        buffer_id: first.buffer_id,
                        start_row: first.row,
                        end_row: last_row,
                        depth: next_depth,
                        tab_size: first.tab_size,
                        settings: first.settings,
                    });
                }
            }
            for guide in &mut stack {
                guide.end_row = last_row;
            }
        }
        guides.extend(stack);
        guides
    }
}
