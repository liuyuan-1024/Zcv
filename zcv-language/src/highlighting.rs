//! 语法高亮：按层 capture 流归并构建跨度。
//!
//! 高亮查询的结果是快照全局 capture 索引（跨主语言与注入语言唯一），渲染侧按索引查样式表，不再解析 capture 名。
//!
//! 每层（主层 + 注入层）单独产出按偏移有序的 capture 事件流（tree-sitter 天然按文档序产出），然后 k 路归并 + 全局活动栈扫描直接产出 spans——不再构造全局事件数组、全量排序或维护 BTreeMap。

use std::cmp::Reverse;
use std::collections::BinaryHeap;
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

/// 单层 capture 事件流中的一个事件（该层内按偏移有序）。
#[derive(Clone, Copy, Debug)]
struct CaptureEvent {
    offset: usize,
    is_start: bool,
    /// 快照全局 capture 名字表的索引。
    capture: u32,
}

/// 归并队列项：按 (偏移, 事件类型, 深度序, 层序) 排序。
///
/// 同偏移下 End 先于 Start；
/// Start 按深度升序（浅层先入栈，深层最后入栈成为栈顶），End 按深度降序（与入栈顺序对称）。
/// 层序保证同层同偏移事件的稳定顺序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct QueueKey {
    offset: usize,
    is_start: bool,
    depth_rank: u32,
    seq: usize,
}

impl QueueKey {
    fn new(event: &CaptureEvent, depth: u32, seq: usize) -> Self {
        // Start：rank = depth（浅层先）；End：rank = MAX - depth（深层先）。
        let depth_rank = if event.is_start {
            depth
        } else {
            u32::MAX - depth
        };
        Self {
            offset: event.offset,
            is_start: event.is_start,
            depth_rank,
            seq,
        }
    }
}

impl SyntaxSnapshot {
    /// 查询指定字节范围，并让更内层、后出现的 capture 覆盖外层。
    ///
    /// 每层一个 capture 流（文档序），k 路归并后以全局活动栈直接产出 spans：
    /// 树中节点要么嵌套要么不相交，注入层 capture 又受其内容节点约束，因此全局栈的 LIFO 顺序就是覆盖顺序，栈顶即当前最内层。
    pub fn highlights(&self, range: Range<usize>, text: &Snapshot) -> Vec<HighlightSpan> {
        if text.version() != self.version || range.start >= range.end {
            return Vec::new();
        }
        let (Some(language), Some(tree)) = (&self.language, self.root_tree()) else {
            return Vec::new();
        };

        // 相关层（主层 + 与范围相交的注入层），每层产出有序事件流。
        let mut streams: Vec<(u32, Vec<CaptureEvent>)> = Vec::new();
        if let Some(events) = collect_capture_events(language, tree, &range, text, self) {
            streams.push((0, events));
        }
        let mut injections: Vec<_> = self
            .injection_layers()
            .iter()
            .filter(|layer| ranges_overlap(&layer.range, &range))
            .map(|layer| (layer.depth, &layer.language, &layer.tree))
            .collect();
        injections.sort_unstable_by_key(|(depth, _, _)| *depth);
        for (depth, language, tree) in injections {
            if let Some(events) = collect_capture_events(language, tree, &range, text, self) {
                streams.push((depth, events));
            }
        }

        sweep_events(streams)
    }
}

/// 在单层树上执行高亮查询，把 capture 裁剪到查询范围并产出有序事件流。
///
/// capture 的 Start/End 成对出现且按偏移有序（tree-sitter 按文档序产出 capture）；
/// 裁剪保证范围内每个 capture 都是完整的一对，扫描期无需处理半开区间。
fn collect_capture_events(
    language: &Language,
    tree: &tree_sitter::Tree,
    range: &Range<usize>,
    text: &Snapshot,
    snapshot: &SyntaxSnapshot,
) -> Option<Vec<CaptureEvent>> {
    if range.start >= range.end {
        return None;
    }
    // 局部 capture index → 快照全局 index 的映射在解析时构建，这里循环外取一次。
    let capture_table = snapshot.capture_index_table(language)?;
    // 无高亮查询的语言不产出 capture。
    let highlights = language.highlights()?;
    let mut cursor = QueryCursorHandle::new();
    cursor.set_byte_range(range.clone());
    let mut events = Vec::new();
    let mut captures = cursor.captures(highlights, tree.root_node(), SnapshotTextProvider(text));
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
            events.push(CaptureEvent {
                offset: start,
                is_start: true,
                capture: global_capture,
            });
            events.push(CaptureEvent {
                offset: end,
                is_start: false,
                capture: global_capture,
            });
        }
    }
    Some(events)
}

/// k 路归并各层事件流，用全局活动栈直接产出非重叠 spans。
///
/// 栈的 LIFO 顺序即覆盖顺序：同偏移先出栈（End）后入栈（Start），同一偏移组结束后栈顶就是该区间的最内层 capture。
fn sweep_events(streams: Vec<(u32, Vec<CaptureEvent>)>) -> Vec<HighlightSpan> {
    let mut heap: BinaryHeap<(Reverse<QueueKey>, usize)> = BinaryHeap::new();
    for (seq, (depth, stream)) in streams.iter().enumerate() {
        if let Some(event) = stream.first() {
            heap.push((Reverse(QueueKey::new(event, *depth, seq)), seq));
        }
    }
    let mut cursors = vec![0usize; streams.len()];
    let mut stack: Vec<u32> = Vec::new();
    let mut spans: Vec<HighlightSpan> = Vec::new();
    let mut pending_offset: Option<usize> = None;
    while let Some((Reverse(key), seq)) = heap.pop() {
        let offset = key.offset;
        // 上一组结束后的活动栈顶，覆盖 [pending_offset, offset) 区间。
        if let Some(prev) = pending_offset
            && prev < offset
            && let Some(&capture) = stack.last()
        {
            if let Some(last) = spans.last_mut()
                && last.range.end == prev
                && last.capture == capture
            {
                last.range.end = offset;
            } else {
                spans.push(HighlightSpan {
                    range: prev..offset,
                    capture,
                });
            }
        }
        // 应用本组事件：End 出栈、Start 入栈（同偏移下 End 先于 Start）。
        let event = streams[seq].1[cursors[seq]];
        if event.is_start {
            stack.push(event.capture);
        } else {
            stack.pop();
        }
        pending_offset = Some(offset);
        // 推进该层游标。
        cursors[seq] += 1;
        if let Some(next) = streams[seq].1.get(cursors[seq]) {
            heap.push((Reverse(QueueKey::new(next, streams[seq].0, seq)), seq));
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use crate::syntax_map::{parsed_syntax, rust_buffer};

    #[test]
    fn double_capture_on_same_node_resolves_to_the_inner_span() {
        // 同一节点命中多个 pattern（function_item 与 identifier 都覆盖函数名）：
        // 归并扫描必须稳定产出内层 capture，且相邻 spans 不重叠。
        let source = "fn main() {}\n";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax_snapshot = syntax.snapshot();
        let names = syntax_snapshot.capture_names();
        let spans = syntax_snapshot.highlights(0..snapshot.len_bytes().get(), &snapshot);

        let main = source.find("main").unwrap();
        let covering = spans
            .iter()
            .filter(|span| span.range.start <= main && main < span.range.end)
            .collect::<Vec<_>>();
        assert_eq!(covering.len(), 1, "函数名区间只应被一个 span 覆盖");
        assert_eq!(
            names[covering[0].capture as usize].as_ref(),
            "function",
            "同节点双捕获应解析为函数名 capture"
        );
        // 相邻 spans 无重叠（含同偏移 End/Start 邻接）。
        for pair in spans.windows(2) {
            assert!(
                pair[0].range.end <= pair[1].range.start,
                "相邻高亮 spans 不应重叠"
            );
        }
    }

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
                .injection_layers()
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
                .injection_layers()
                .iter()
                .any(|layer| layer.language.name() == "CSS")
        );
        assert!(
            syntax
                .injection_layers()
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
