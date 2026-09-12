//! 语法缩进查询。
//!
//! `indent.scm` 只描述语法结构造成的额外缩进；
//! 基准行的已有空白仍从文本快照读取，因而编辑器可以按自身的 Tab 配置决定最终插入字符。

use std::ops::Range;

use tree_sitter::StreamingIterator;
use zcv_text::{ByteOffset, Line, Snapshot, TextResult};

use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::{QueryCursorHandle, SnapshotTextProvider};

/// 语法结构产生的缩进范围。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndentRange {
    pub range: Range<usize>,
    pub end: Option<Range<usize>>,
}

/// 在指定光标位置按 Enter 后，目标行应采用的缩进。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewlineIndent {
    pub base_indent: String,
    pub additional_levels: usize,
}

impl SyntaxSnapshot {
    /// 查询范围内由 `indent.scm` 声明的缩进结构。
    pub fn indent_ranges(&self, range: Range<usize>, text: &Snapshot) -> Vec<IndentRange> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        let mut ranges = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.indents() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some(query_match) = matches.next() {
                let mut indent = None;
                let mut end = None;
                for capture in query_match.captures {
                    match names.get(capture.index as usize).copied() {
                        Some("indent") => indent = Some(capture.node.byte_range()),
                        Some("end") => end = Some(capture.node.byte_range()),
                        _ => {}
                    }
                }
                if let Some(range) = indent {
                    ranges.push(IndentRange { range, end });
                }
            }
        }
        ranges.sort_unstable_by_key(|range| (range.range.start, range.range.end));
        ranges
    }

    /// 基于语言语法树计算在 `offset` 处换行时，下一行的建议缩进。
    ///
    /// 语言层负责找到缩进基准和未闭合的语法结构，编辑器负责将结果应用到插入文本。
    pub fn suggested_newline_indent(
        &self,
        offset: ByteOffset,
        text: &Snapshot,
    ) -> TextResult<NewlineIndent> {
        let current_line = text.byte_to_line(offset)?;
        let line_start = text.line_start_byte(current_line)?;
        let prefix = text.slice_byte_range(line_start, offset)?;
        let (basis_line, base_indent) = newline_indent_basis(text, current_line, prefix.as_str())?;
        let query_start = offset.get().saturating_sub(1);
        let query_end = offset.get().saturating_add(1).min(text.len_bytes().get());
        let additional_levels = usize::from(
            self.indent_ranges(query_start..query_end, text)
                .into_iter()
                .any(|range| {
                    text.byte_to_line(ByteOffset::new(range.range.start)) == Ok(basis_line)
                        && range.range.start < offset.get()
                        && offset.get() < range.range.end
                        && range
                            .end
                            .as_ref()
                            .is_none_or(|end| offset.get() <= end.start)
                }),
        );
        Ok(NewlineIndent {
            base_indent,
            additional_levels,
        })
    }
}

fn newline_indent_basis(
    text: &Snapshot,
    current_line: Line,
    prefix: &str,
) -> TextResult<(Line, String)> {
    if prefix
        .chars()
        .any(|character| !matches!(character, ' ' | '\t'))
    {
        return Ok((current_line, leading_whitespace(prefix)));
    }

    for line_index in (0..current_line.get()).rev() {
        let line = Line::new(line_index);
        let content = text.line_content(line, None)?;
        if content
            .as_str()
            .chars()
            .any(|character| !matches!(character, ' ' | '\t'))
        {
            return Ok((line, leading_whitespace(content.as_str())));
        }
    }

    Ok((current_line, leading_whitespace(prefix)))
}

fn leading_whitespace(text: &str) -> String {
    text.chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect()
}
