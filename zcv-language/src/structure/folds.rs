//! 结构折叠查询。
//!
//! `fold.scm` 声明可折叠的语法区域，模块再把它们规整为编辑器使用的源字节范围。
//! 折叠范围隐藏入口行之后的内容，但保留闭合定界符，以便显示层保持结构可读性。

use std::ops::Range;

use tree_sitter::StreamingIterator;
use zcv_text::{ByteOffset, Line, Snapshot};

use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::{QueryCursorHandle, SnapshotTextProvider};

/// 一个可折叠的源文本范围。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldRange {
    pub range: Range<usize>,
}

impl SyntaxSnapshot {
    /// 查询范围内的折叠区域，并按源文本起点排序。
    pub fn fold_ranges(&self, range: Range<usize>, text: &Snapshot) -> Vec<FoldRange> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        // 折叠查询的约定：@fold 指定语法区域；需要保留闭合符号的区域以 @fold.end 声明其边界。
        let mut nodes = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.folds() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some(query_match) = matches.next() {
                let explicit_end = query_match
                    .captures
                    .iter()
                    .find(|capture| {
                        names
                            .get(capture.index as usize)
                            .is_some_and(|name| &**name == "fold.end")
                    })
                    .map(|capture| capture.node.byte_range().start);

                // 同一个 match 命中多个节点时，行相邻则合并成一个折叠范围。
                let mut captured: Vec<_> = query_match
                    .captures
                    .iter()
                    .filter(|capture| {
                        names
                            .get(capture.index as usize)
                            .is_some_and(|name| &**name == "fold")
                    })
                    .map(|capture| (capture.node, explicit_end))
                    .collect();
                captured.sort_unstable_by_key(|(node, _)| node.byte_range().start);
                let mut merged: Vec<(Range<usize>, usize, usize, Option<usize>)> = Vec::new();
                for (node, explicit_end) in captured {
                    let byte_range = node.byte_range();
                    match merged.last_mut() {
                        Some((range, _, end_row, _))
                            if node.start_position().row <= *end_row + 1 =>
                        {
                            range.end = range.end.max(byte_range.end);
                            *end_row = node.end_position().row;
                        }
                        _ => {
                            merged.push((
                                byte_range,
                                node.start_position().row,
                                node.end_position().row,
                                explicit_end,
                            ));
                        }
                    }
                }
                nodes.extend(merged);
            }
        }

        // 定界符：把折叠范围重塑为 [入口行行尾换行符, 闭合符号前)，闭合符号保留可见。
        let pairs = self.bracket_pairs(range.clone(), text);
        let mut ranges = Vec::new();
        for (byte_range, _, _, explicit_end) in nodes {
            let Ok(anchor_line) = text.byte_to_line(ByteOffset::new(byte_range.start)) else {
                continue;
            };
            let start = line_newline_position(text, anchor_line);
            let delimiter_end = pairs
                .iter()
                .filter(|pair| pair.open.start == byte_range.start)
                .filter(|pair| pair.close.end <= byte_range.end)
                .filter(|pair| {
                    text.byte_to_line(ByteOffset::new(pair.close.start))
                        .is_ok_and(|line| line > anchor_line)
                })
                .map(|pair| pair.close.start)
                .max();
            let end = explicit_end
                .or(delimiter_end)
                .map(ByteOffset::new)
                .unwrap_or_else(|| {
                    let mut end_line = text
                        .byte_to_line(ByteOffset::new(byte_range.end))
                        .unwrap_or(anchor_line);
                    if end_line > anchor_line
                        && text
                            .line_start_byte(end_line)
                            .is_ok_and(|start| start.get() == byte_range.end)
                    {
                        end_line = Line::new(end_line.get() - 1);
                    }
                    line_content_end(text, end_line)
                });
            if start >= end || text.byte_to_line(end).is_ok_and(|line| line <= anchor_line) {
                continue;
            }
            ranges.push(FoldRange {
                range: start.get()..end.get(),
            });
        }
        ranges.sort_unstable_by_key(|range| (range.range.start, range.range.end));
        ranges
    }
}

/// 行终止换行符的字节位置；行尾无换行符时返回行尾。
fn line_newline_position(text: &Snapshot, line: Line) -> ByteOffset {
    let content = text
        .line_content(line, None)
        .expect("折叠入口行必须位于当前 Snapshot 内");
    if content.text_range().end() == content.full_range().end() {
        content.full_range().end()
    } else {
        ByteOffset::new(content.full_range().end().get() - 1)
    }
}

/// 行内容末尾（不含终止换行符）。
fn line_content_end(text: &Snapshot, line: Line) -> ByteOffset {
    text.line_content(line, None)
        .expect("折叠末行必须位于当前 Snapshot 内")
        .text_range()
        .end()
}
