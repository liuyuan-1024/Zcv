//! 文件符号大纲查询。
//!
//! 查询只消费语言注册的 `outline.scm`，不通过文本扫描猜测符号。
//! 主语言和注入语言分别查询，再按源坐标的范围包含关系计算层级。

use std::ops::Range;
use std::sync::Arc;

use tree_sitter::StreamingIterator;
use zcv_text::{Affinity, Anchor, ByteOffset, Snapshot};

use crate::HighlightCache;

use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::{QueryCursorHandle, SnapshotTextProvider, encloses, node_text};

/// 大纲生成期间的标签片段与源代码对应关系。
#[derive(Clone, Debug, PartialEq, Eq)]
struct OutlineLabelPart {
    text_range: Range<usize>,
    source_range: Range<usize>,
}

/// 文件级语法大纲项。
///
/// 位置由源 Anchor 表示；标签和相对高亮只依赖生成时的同一文本快照。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem<T = Anchor> {
    /// 定义节点的完整范围。
    pub range: Range<T>,
    /// 用户点击大纲项时应定位的名称范围。
    pub name_range: Range<T>,
    /// 标签各源片段的最小覆盖范围。
    pub source_range_for_text: Range<T>,
    /// 定义名称文本；无法稳定识别名称的查询命中不会生成大纲项。
    pub name: String,
    /// 大纲行显示文本。
    pub text: String,
    /// 标签内的相对字节范围和生成时捕获的语法名称。
    pub highlight_ranges: Vec<(Range<usize>, Arc<str>)>,
    /// 标签内定义名称的相对字节范围。
    pub name_ranges: Vec<Range<usize>>,
    /// Tree-sitter 定义节点种类。
    pub kind: String,
    /// 按范围包含关系计算的层级，顶层为 0。
    pub depth: usize,
    /// 产生该项的语法层名称。
    pub language: &'static str,
    /// 产生该项的注入层深度，主语言层为 0。
    pub language_depth: u32,
    /// 定义主体范围；查询没有声明 `@open`/`@close` 时为空。
    pub body_range: Option<Range<T>>,
    /// 紧邻定义之前的属性、装饰器或文档注释范围。
    pub annotation_range: Option<Range<T>>,
}

#[derive(Clone, Debug)]
struct OutlineCandidate {
    item: OutlineItem<usize>,
}

impl SyntaxSnapshot {
    /// 查询指定范围内的文件级符号，并根据定义范围的包含关系建立父子层级。
    ///
    /// 查询缺失时返回空，不通过文本扫描或隐式默认规则伪造符号。
    /// 主语言和实际相交的注入层都会被查询，Tree-sitter 注入树已经使用源文件字节坐标，因此结果无需二次偏移。
    pub fn outline(
        &self,
        range: Range<usize>,
        text: &Snapshot,
        highlight_cache: &HighlightCache,
    ) -> Vec<OutlineItem> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }

        let mut candidates = Vec::new();
        let mut annotations = Vec::new();
        let capture_names = self.capture_names();
        for layer in self.layers_for_range(text, &range) {
            let Some(query) = layer.language.outline() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some(query_match) = matches.next() {
                let mut items = Vec::new();
                let mut names_in_match = Vec::new();
                let mut contexts = Vec::new();
                let mut opens = Vec::new();
                let mut closes = Vec::new();
                for capture in query_match.captures {
                    let node = capture.node;
                    let node_range = node.byte_range();
                    match names.get(capture.index as usize).copied() {
                        Some("item") => items.push(node),
                        Some("name") => names_in_match.push(node),
                        Some("context") | Some("context.extra") => contexts.push(node),
                        Some("annotation") => {
                            annotations.push((layer.depth, layer.language.name(), node_range));
                        }
                        Some("open") => opens.push(node_range),
                        Some("close") => closes.push(node_range),
                        _ => {}
                    }
                }

                for item_node in items {
                    let item_range = item_node.byte_range();
                    let Some(name_node) = names_in_match
                        .iter()
                        .find(|node| encloses(&item_range, &node.byte_range()))
                    else {
                        continue;
                    };
                    let name_range = name_node.byte_range();
                    let Some(name) = node_text(text, name_range.clone()) else {
                        continue;
                    };
                    if name.is_empty() {
                        continue;
                    }
                    let (label, label_parts) =
                        outline_text(text, &contexts, *name_node, &item_range, &name);
                    let name_ranges = label_parts
                        .iter()
                        .filter(|part| part.source_range == name_range)
                        .map(|part| part.text_range.clone())
                        .collect();
                    let source_range_for_text = label_parts
                        .iter()
                        .map(|part| part.source_range.clone())
                        .reduce(|left, right| left.start.min(right.start)..left.end.max(right.end))
                        .unwrap_or_else(|| name_range.clone());
                    let highlight_ranges = label_parts
                        .iter()
                        .flat_map(|part| {
                            self.highlights(part.source_range.clone(), text, highlight_cache)
                                .into_iter()
                                .filter_map(|span| {
                                    capture_names.get(span.capture as usize).map(|name| {
                                        let start = part.text_range.start + span.range.start
                                            - part.source_range.start;
                                        let end = part.text_range.start + span.range.end
                                            - part.source_range.start;
                                        (start..end, Arc::clone(name))
                                    })
                                })
                        })
                        .collect();
                    let body_range = paired_body_range(&opens, &closes, &item_range);
                    candidates.push(OutlineCandidate {
                        item: OutlineItem {
                            range: item_range,
                            name_range,
                            source_range_for_text,
                            name,
                            text: label,
                            highlight_ranges,
                            name_ranges,
                            kind: item_node.kind().to_owned(),
                            depth: 0,
                            language: layer.language.name(),
                            language_depth: layer.depth,
                            body_range,
                            annotation_range: None,
                        },
                    });
                }
            }
        }

        candidates.sort_unstable_by_key(|candidate| {
            (
                candidate.item.range.start,
                usize::MAX - candidate.item.range.end,
                candidate.item.name_range.start,
            )
        });
        candidates.dedup_by(|left, right| {
            left.item.range == right.item.range
                && left.item.name_range == right.item.name_range
                && left.item.name == right.item.name
                && left.item.language == right.item.language
        });

        let mut items: Vec<OutlineItem<usize>> = candidates
            .into_iter()
            .map(|candidate| candidate.item)
            .collect();
        for index in 0..items.len() {
            let parent = (0..index)
                .filter(|&candidate| {
                    candidate != index
                        && encloses(&items[candidate].range, &items[index].range)
                        && items[candidate].range != items[index].range
                })
                .min_by_key(|&candidate| items[candidate].range.len());
            items[index].depth = parent.map_or(0, |parent| items[parent].depth + 1);
            items[index].annotation_range = annotation_range_for(
                &annotations,
                items[index].range.start,
                items[index].language,
                items[index].language_depth,
            );
        }
        items
            .into_iter()
            .map(|item| {
                let anchor_range = |range: Range<usize>| {
                    text.anchor_with_affinity(ByteOffset::new(range.start), Affinity::After)
                        ..text.anchor_with_affinity(ByteOffset::new(range.end), Affinity::Before)
                };
                OutlineItem {
                    range: anchor_range(item.range),
                    name_range: anchor_range(item.name_range),
                    source_range_for_text: anchor_range(item.source_range_for_text),
                    name: item.name,
                    text: item.text,
                    highlight_ranges: item.highlight_ranges,
                    name_ranges: item.name_ranges,
                    kind: item.kind,
                    depth: item.depth,
                    language: item.language,
                    language_depth: item.language_depth,
                    body_range: item.body_range.map(anchor_range),
                    annotation_range: item.annotation_range.map(anchor_range),
                }
            })
            .collect()
    }
}

fn outline_text(
    text: &Snapshot,
    nodes: &[tree_sitter::Node<'_>],
    name_node: tree_sitter::Node<'_>,
    item_range: &Range<usize>,
    name: &str,
) -> (String, Vec<OutlineLabelPart>) {
    let mut ranges: Vec<_> = nodes
        .iter()
        .map(|node| node.byte_range())
        .filter(|range| encloses(item_range, range))
        .collect();
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    let mut outline_text = String::new();
    let mut parts = Vec::new();
    let mut append = |range: Range<usize>, value: &str| {
        if value.is_empty() {
            return;
        }
        if !outline_text.is_empty() {
            outline_text.push(' ');
        }
        let text_start = outline_text.len();
        outline_text.push_str(value);
        parts.push(OutlineLabelPart {
            text_range: text_start..outline_text.len(),
            source_range: range,
        });
    };
    for range in ranges {
        let Some(value) = node_text(text, range.clone()) else {
            continue;
        };
        append(range, &value);
    }
    append(name_node.byte_range(), name);
    (outline_text, parts)
}

fn paired_body_range(
    opens: &[Range<usize>],
    closes: &[Range<usize>],
    item_range: &Range<usize>,
) -> Option<Range<usize>> {
    let open = opens
        .iter()
        .filter(|range| encloses(item_range, range))
        .min_by_key(|range| range.start)?;
    let close = closes
        .iter()
        .filter(|range| encloses(item_range, range) && range.start >= open.end)
        .min_by_key(|range| range.start)?;
    (open.end <= close.start).then_some(open.end..close.start)
}

fn annotation_range_for(
    annotations: &[(u32, &'static str, Range<usize>)],
    item_start: usize,
    language: &'static str,
    language_depth: u32,
) -> Option<Range<usize>> {
    let mut annotations: Vec<_> = annotations
        .iter()
        .filter(|(depth, name, range)| {
            *depth == language_depth && *name == language && range.end <= item_start
        })
        .map(|(_, _, range)| range.clone())
        .collect();
    annotations.sort_unstable_by_key(|range| (range.start, range.end));
    let last = annotations.last()?.clone();
    if item_start.saturating_sub(last.end) > 1 {
        return None;
    }
    let start = annotations
        .iter()
        .rev()
        .take_while(|range| item_start.saturating_sub(range.end) <= 1)
        .map(|range| range.start)
        .min()
        .unwrap_or(last.start);
    Some(start..last.end)
}
