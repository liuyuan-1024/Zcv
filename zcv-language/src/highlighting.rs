//! 语法高亮：capture 事件收集与跨度构建。
//!
//! 高亮查询的结果是快照全局 capture 索引（跨主语言与注入语言唯一），渲染侧按索引查样式表，不再解析 capture 名。

use std::collections::BTreeMap;
use std::ops::Range;

use tree_sitter::StreamingIterator;
use zcv_text::Snapshot;

use crate::Language;
use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::{QueryCursorHandle, SnapshotTextProvider, ranges_overlap};

/// 一个非重叠的 tree-sitter capture 区间。
///
/// `capture` 是快照全局 capture 名字表的索引（跨主语言与注入语言唯一），渲染侧按索引查预展开的样式表，不再携带并逐 run 解析 capture 名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub capture: u32,
}

#[derive(Clone)]
enum HighlightEvent {
    Start {
        offset: usize,
        ordinal: usize,
        /// 快照全局 capture 名字表的索引。
        capture: u32,
    },
    End {
        offset: usize,
        ordinal: usize,
    },
}

impl HighlightEvent {
    fn offset(&self) -> usize {
        match self {
            Self::Start { offset, .. } | Self::End { offset, .. } => *offset,
        }
    }

    fn sort_key(&self) -> (usize, bool, usize) {
        match self {
            Self::End {
                offset, ordinal, ..
            } => (*offset, false, *ordinal),
            Self::Start {
                offset, ordinal, ..
            } => (*offset, true, *ordinal),
        }
    }
}

impl SyntaxSnapshot {
    /// 查询指定字节范围，并像 Zed 的 BufferChunks 一样让更内层、后出现的 capture 覆盖外层。
    pub fn highlights(&self, range: Range<usize>, text: &Snapshot) -> Vec<HighlightSpan> {
        if text.version() != self.version || range.start >= range.end {
            return Vec::new();
        }
        let (Some(language), Some(tree)) = (&self.language, &self.tree) else {
            return Vec::new();
        };

        let mut events = Vec::new();
        let mut ordinal = 0usize;
        collect_highlight_events(
            language,
            tree,
            range.clone(),
            text,
            self,
            &mut ordinal,
            &mut events,
        );
        let mut injections: Vec<_> = self
            .injections
            .iter()
            .filter(|layer| ranges_overlap(&layer.range, &range))
            .collect();
        injections.sort_unstable_by_key(|layer| layer.depth);
        for layer in injections {
            let layer_range = layer.range.start.max(range.start)..layer.range.end.min(range.end);
            collect_highlight_events(
                &layer.language,
                &layer.tree,
                layer_range,
                text,
                self,
                &mut ordinal,
                &mut events,
            );
        }
        events.sort_unstable_by_key(HighlightEvent::sort_key);

        let mut active = BTreeMap::new();
        let mut spans: Vec<HighlightSpan> = Vec::new();
        let mut index = 0;
        while index < events.len() {
            let offset = events[index].offset();
            while index < events.len() && events[index].offset() == offset {
                match &events[index] {
                    HighlightEvent::Start {
                        ordinal, capture, ..
                    } => {
                        active.insert(*ordinal, *capture);
                    }
                    HighlightEvent::End { ordinal, .. } => {
                        active.remove(ordinal);
                    }
                }
                index += 1;
            }
            let Some(next_offset) = events.get(index).map(HighlightEvent::offset) else {
                break;
            };
            let Some((_, capture)) = active.last_key_value() else {
                continue;
            };
            if offset < next_offset {
                if let Some(last) = spans.last_mut()
                    && last.range.end == offset
                    && last.capture == *capture
                {
                    last.range.end = next_offset;
                } else {
                    spans.push(HighlightSpan {
                        range: offset..next_offset,
                        capture: *capture,
                    });
                }
            }
        }
        spans
    }
}

fn collect_highlight_events(
    language: &Language,
    tree: &tree_sitter::Tree,
    range: Range<usize>,
    text: &Snapshot,
    snapshot: &SyntaxSnapshot,
    ordinal: &mut usize,
    events: &mut Vec<HighlightEvent>,
) {
    if range.start >= range.end {
        return;
    }
    // 局部 capture index → 快照全局 index 的映射在解析时构建，这里循环外取一次。
    let Some(capture_table) = snapshot.capture_index_table(language) else {
        return;
    };
    let mut cursor = QueryCursorHandle::new();
    cursor.set_byte_range(range.clone());
    let mut captures = cursor.captures(
        language.highlights(),
        tree.root_node(),
        SnapshotTextProvider(text),
    );
    while let Some((query_match, capture_index)) = captures.next() {
        let capture = query_match.captures[*capture_index];
        let capture_range = capture.node.byte_range();
        let start = capture_range.start.max(range.start);
        let end = capture_range.end.min(range.end);
        // 注入层的 capture 也经此表映射，渲染侧统一查全局表。
        let Some(&global_capture) = capture_table.get(capture.index as usize) else {
            continue;
        };
        if start < end {
            events.push(HighlightEvent::Start {
                offset: start,
                ordinal: *ordinal,
                capture: global_capture,
            });
            events.push(HighlightEvent::End {
                offset: end,
                ordinal: *ordinal,
            });
            *ordinal += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::syntax_map::{parsed_syntax, rust_buffer};

    #[test]
    fn highlights_rust_captures_in_unicode_text() {
        let (buffer, syntax) = rust_buffer("fn 问候() { let 文本 = \"你好\"; }\n");
        let snapshot = buffer.snapshot();
        let syntax_snapshot = syntax.snapshot();
        let names = syntax_snapshot.capture_names();
        let spans = syntax_snapshot.highlights(0..snapshot.len_bytes().get(), &snapshot);

        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "keyword")
        );
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "function")
        );
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "string")
        );
        assert!(
            spans
                .iter()
                .all(|span| span.range.end <= snapshot.len_bytes().get())
        );
    }

    #[test]
    fn markdown_inline_layer_overrides_block_highlights() {
        let (buffer, syntax) = parsed_syntax("README.md", "普通 *强调* 和 **加粗**\n");
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        assert!(
            syntax
                .injections
                .iter()
                .any(|layer| layer.language.name() == "Markdown Inline")
        );
        let names = syntax.capture_names();
        let spans = syntax.highlights(0..snapshot.len_bytes().get(), &snapshot);
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "text.emphasis")
        );
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "text.strong")
        );
    }

    #[test]
    fn html_injects_css_and_javascript_layers() {
        let source = "<style>.item { color: red; }</style><script>let value = 1;</script>";
        let (buffer, syntax) = parsed_syntax("index.html", source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        assert!(
            syntax
                .injections
                .iter()
                .any(|layer| layer.language.name() == "CSS")
        );
        assert!(
            syntax
                .injections
                .iter()
                .any(|layer| layer.language.name() == "JavaScript")
        );
        let names = syntax.capture_names();
        let spans = syntax.highlights(0..snapshot.len_bytes().get(), &snapshot);
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "property")
        );
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "keyword")
        );
    }
}
