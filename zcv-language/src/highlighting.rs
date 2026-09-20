//! 语法高亮：按层 capture 流归并构建跨度。
//!
//! 高亮查询的结果是快照全局 capture 索引（跨主语言与注入语言唯一），渲染侧按索引查样式表，不再解析 capture 名。
//!
//! 每层（主层 + 注入层）单独产出按文档序排列的 capture 区间，然后 k 路归并 capture 起点；
//! 活动栈用区间终点恢复外层高亮，避免为结束位置额外构造一份全局事件数组。

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ops::Range;
use std::sync::Arc;

use tree_sitter::StreamingIterator;
use zcv_text::Snapshot;

use crate::Language;
use crate::highlight_cache::HighlightCache;
use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::{
    ParseCancellation, QueryCursorHandle, SnapshotTextProvider, ranges_overlap,
};

/// 一个非重叠的 tree-sitter capture 区间。
///
/// `capture` 是快照全局 capture 名字表的索引（跨主语言与注入语言唯一），渲染侧按索引查预展开的样式表，不再携带并逐 run 解析 capture 名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub capture: u32,
}

/// 单层 capture 流中的一个区间。
#[derive(Clone, Copy, Debug)]
struct CaptureRange {
    start: usize,
    end: usize,
    capture: u32,
}

/// 归并队列项：起点相同时，外层区间和浅层语法树先入栈，内层高亮最后覆盖。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct QueueKey {
    start: usize,
    end: Reverse<usize>,
    depth: u32,
    seq: usize,
}

impl QueueKey {
    fn new(capture: &CaptureRange, depth: u32, seq: usize) -> Self {
        Self {
            start: capture.start,
            end: Reverse(capture.end),
            depth,
            seq,
        }
    }
}

impl SyntaxSnapshot {
    /// 查询指定字节范围，并让更内层、后出现的 capture 覆盖外层。
    ///
    /// 每层一个 capture 流（文档序），k 路归并后以全局活动栈直接产出 spans：
    /// 树中节点要么嵌套要么不相交，注入层 capture 又受其内容节点约束，因此全局栈的 LIFO 顺序就是覆盖顺序，栈顶即当前最内层。
    pub fn highlights(
        &self,
        range: Range<usize>,
        text: &Snapshot,
        cache: &HighlightCache,
    ) -> Vec<HighlightSpan> {
        if range.start >= range.end || text.version() != self.version {
            return Vec::new();
        }
        const CACHE_CHUNK_BYTES: usize = 4096;
        let first = range.start / CACHE_CHUNK_BYTES * CACHE_CHUNK_BYTES;
        let end = range.end.min(text.len_bytes().get());
        let mut spans = Vec::new();
        let mut chunk_start = first;
        while chunk_start < end {
            let chunk_end = (chunk_start + CACHE_CHUNK_BYTES).min(text.len_bytes().get());
            let cached = cache.get(chunk_start).unwrap_or_else(|| {
                let computed = Arc::from(
                    self.highlights_impl(chunk_start..chunk_end, text, None)
                        .unwrap_or_default()
                        .into_boxed_slice(),
                );
                cache.insert(chunk_start, Arc::clone(&computed));
                computed
            });
            spans.extend(cached.iter().filter_map(|span| {
                let start = span.range.start.max(range.start);
                let end = span.range.end.min(range.end);
                (start < end).then_some(HighlightSpan {
                    range: start..end,
                    capture: span.capture,
                })
            }));
            chunk_start = chunk_end;
        }
        spans
    }

    /// 查询指定字节范围，并允许后台消费者放弃过期高亮任务。
    pub(crate) fn highlights_with_cancellation(
        &self,
        range: Range<usize>,
        text: &Snapshot,
        cancellation: &ParseCancellation,
    ) -> Option<Vec<HighlightSpan>> {
        self.highlights_impl(range, text, Some(cancellation))
    }

    fn highlights_impl(
        &self,
        range: Range<usize>,
        text: &Snapshot,
        cancellation: Option<&ParseCancellation>,
    ) -> Option<Vec<HighlightSpan>> {
        if cancellation.is_some_and(ParseCancellation::is_cancelled) {
            return None;
        }
        if text.version() != self.version || range.start >= range.end {
            return Some(Vec::new());
        }
        let (Some(language), Some(tree)) = (&self.language, self.root_tree()) else {
            return Some(Vec::new());
        };

        // 相关层（主层 + 与范围相交的注入层），每层产出有序 capture 流。
        let mut streams: Vec<(u32, Vec<CaptureRange>)> = Vec::new();
        if let Some(captures) =
            collect_capture_ranges(language, tree, &range, text, self, cancellation)?
        {
            streams.push((0, captures));
        }
        let mut injections: Vec<_> = self
            .injection_layers()
            .iter()
            .filter(|layer| ranges_overlap(&layer.range, &range))
            .map(|layer| (layer.depth, &layer.language, &layer.tree))
            .collect();
        injections.sort_unstable_by_key(|(depth, _, _)| *depth);
        for (depth, language, tree) in injections {
            if let Some(captures) =
                collect_capture_ranges(language, tree, &range, text, self, cancellation)?
            {
                streams.push((depth, captures));
            }
        }

        sweep_captures(streams, range.end, cancellation)
    }
}

/// 在单层树上执行高亮查询，把 capture 裁剪到查询范围并保留 Tree-sitter 的文档顺序。
fn collect_capture_ranges(
    language: &Language,
    tree: &tree_sitter::Tree,
    range: &Range<usize>,
    text: &Snapshot,
    snapshot: &SyntaxSnapshot,
    cancellation: Option<&ParseCancellation>,
) -> Option<Option<Vec<CaptureRange>>> {
    if cancellation.is_some_and(ParseCancellation::is_cancelled) {
        return None;
    }
    if range.start >= range.end {
        return Some(None);
    }
    // 局部 capture index → 快照全局 index 的映射在解析时构建，这里循环外取一次。
    let Some(capture_table) = snapshot.capture_index_table(language) else {
        return Some(None);
    };
    // 无高亮查询的语言不产出 capture。
    let Some(highlights) = language.highlights() else {
        return Some(None);
    };
    let mut cursor = QueryCursorHandle::new();
    cursor.set_byte_range(range.clone());
    let mut ranges = Vec::new();
    let mut captures = cursor.captures(highlights, tree.root_node(), SnapshotTextProvider(text));
    while let Some((query_match, capture_index)) = captures.next() {
        if cancellation.is_some_and(ParseCancellation::is_cancelled) {
            return None;
        }
        let capture = query_match.captures[*capture_index];
        let capture_range = capture.node.byte_range();
        let start = capture_range.start.max(range.start);
        let end = capture_range.end.min(range.end);
        let Some(name) = language.capture_names().get(capture.index as usize) else {
            continue;
        };
        if name.starts_with('_') {
            continue;
        }
        // 注入层的 capture 也经此表映射，渲染侧统一查全局表。
        let Some(&global_capture) = capture_table.get(capture.index as usize) else {
            continue;
        };
        if start < end {
            ranges.push(CaptureRange {
                start,
                end,
                capture: global_capture,
            });
        }
    }
    Some(Some(ranges))
}

/// k 路归并各层 capture 起点，用活动栈直接产出非重叠 spans。
fn sweep_captures(
    streams: Vec<(u32, Vec<CaptureRange>)>,
    range_end: usize,
    cancellation: Option<&ParseCancellation>,
) -> Option<Vec<HighlightSpan>> {
    let mut heap: BinaryHeap<(Reverse<QueueKey>, usize)> = BinaryHeap::new();
    for (seq, (depth, stream)) in streams.iter().enumerate() {
        if let Some(capture) = stream.first() {
            heap.push((Reverse(QueueKey::new(capture, *depth, seq)), seq));
        }
    }
    let mut cursors = vec![0usize; streams.len()];
    let mut stack: Vec<(usize, u32)> = Vec::new();
    let mut spans: Vec<HighlightSpan> = Vec::new();
    let mut offset = heap
        .peek()
        .map(|(Reverse(key), _)| key.start)
        .unwrap_or(range_end);

    while let Some((Reverse(key), seq)) = heap.pop() {
        if cancellation.is_some_and(ParseCancellation::is_cancelled) {
            return None;
        }
        emit_until(key.start, &mut offset, &mut stack, &mut spans);

        let capture = streams[seq].1[cursors[seq]];
        stack.push((capture.end, capture.capture));

        // 推进该层游标。
        cursors[seq] += 1;
        if let Some(next) = streams[seq].1.get(cursors[seq]) {
            heap.push((Reverse(QueueKey::new(next, streams[seq].0, seq)), seq));
        }
    }

    emit_until(range_end, &mut offset, &mut stack, &mut spans);
    Some(spans)
}

fn emit_until(
    target: usize,
    offset: &mut usize,
    stack: &mut Vec<(usize, u32)>,
    spans: &mut Vec<HighlightSpan>,
) {
    while *offset < target {
        while stack.last().is_some_and(|(end, _)| *end <= *offset) {
            stack.pop();
        }
        let Some(&(end, capture)) = stack.last() else {
            *offset = target;
            break;
        };
        let span_end = end.min(target);
        if let Some(last) = spans.last_mut()
            && last.range.end == *offset
            && last.capture == capture
        {
            last.range.end = span_end;
        } else {
            spans.push(HighlightSpan {
                range: *offset..span_end,
                capture,
            });
        }
        *offset = span_end;
    }
}

#[cfg(test)]
#[path = "test/highlighting_tests.rs"]
mod tests;
