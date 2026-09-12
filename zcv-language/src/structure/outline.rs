//! 文件符号大纲查询。
//!
//! 查询只消费语言注册的 `outline.scm`，不通过文本扫描猜测符号。
//! 主语言和注入语言分别查询，再按源坐标的范围包含关系计算层级。

use std::ops::Range;

use tree_sitter::StreamingIterator;
use zcv_text::{BufferVersion, Snapshot};

use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::{QueryCursorHandle, SnapshotTextProvider, encloses, node_text};

/// 大纲文本中一段内容与源代码的对应关系。
///
/// 两个范围均使用 UTF-8 字节偏移；
/// `text_range` 位于 [`OutlineItem::text`]，`source_range` 位于产生该大纲项的源快照。
/// 文本中的分隔空格没有对应的源范围。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineTextRange {
    pub text_range: Range<usize>,
    pub source_range: Range<usize>,
}

/// 文件级语法大纲项。
///
/// 范围对应当前 `SyntaxSnapshot` 的 UTF-8 字节坐标；
/// 该类型不携带显示行或可写状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    /// 产生结果的文本版本。
    pub version: BufferVersion,
    /// 定义节点的完整范围。
    pub range: Range<usize>,
    /// 用户点击大纲项时应定位的名称范围。
    pub name_range: Range<usize>,
    /// 定义名称文本；无法稳定识别名称的查询命中不会生成大纲项。
    pub name: String,
    /// 大纲行显示文本。
    pub text: String,
    /// 显示文本片段到源代码的映射。
    pub text_ranges: Vec<OutlineTextRange>,
    /// Tree-sitter 定义节点种类。
    pub kind: String,
    /// 按范围包含关系计算的层级，顶层为 0。
    pub depth: usize,
    /// 产生该项的语法层名称。
    pub language: &'static str,
    /// 产生该项的注入层深度，主语言层为 0。
    pub language_depth: u32,
    /// 定义主体范围；查询没有声明 `@open`/`@close` 时为空。
    pub body_range: Option<Range<usize>>,
    /// 紧邻定义之前的属性、装饰器或文档注释范围。
    pub annotation_range: Option<Range<usize>>,
}

#[derive(Clone, Debug)]
struct OutlineCandidate {
    item: OutlineItem,
}

impl SyntaxSnapshot {
    /// 查询指定范围内的文件级符号，并根据定义范围的包含关系建立父子层级。
    ///
    /// 查询缺失时返回空，不通过文本扫描或隐式默认规则伪造符号。
    /// 主语言和实际相交的注入层都会被查询，Tree-sitter 注入树已经使用源文件字节坐标，因此结果无需二次偏移。
    pub fn outline(&self, range: Range<usize>, text: &Snapshot) -> Vec<OutlineItem> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }

        let mut candidates = Vec::new();
        let mut annotations = Vec::new();
        for layer in self.layers_for_range(&range) {
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
                    let (text, text_ranges) =
                        outline_text(text, &contexts, *name_node, &item_range, &name);
                    let body_range = paired_body_range(&opens, &closes, &item_range);
                    candidates.push(OutlineCandidate {
                        item: OutlineItem {
                            version: self.version,
                            range: item_range,
                            name_range,
                            name,
                            text,
                            text_ranges,
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

        let mut items: Vec<OutlineItem> = candidates
            .iter()
            .map(|candidate| candidate.item.clone())
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
    }
}

fn outline_text(
    text: &Snapshot,
    nodes: &[tree_sitter::Node<'_>],
    name_node: tree_sitter::Node<'_>,
    item_range: &Range<usize>,
    name: &str,
) -> (String, Vec<OutlineTextRange>) {
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
        parts.push(OutlineTextRange {
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
