//! 结构查询：括号配对、大纲、缩进、折叠、文本对象。
//!
//! 各查询在同一模式的路径上执行：取与范围相交的语法层 → 在每层跑 tree-sitter 查询 → 收集结果并排序。

use std::ops::Range;

use tree_sitter::StreamingIterator;
use zcv_text::{BufferVersion, ByteOffset, Line, Snapshot, TextResult};

use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::{QueryCursorHandle, SnapshotTextProvider, encloses, node_text};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BracketPair {
    pub open: Range<usize>,
    pub close: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndentRange {
    pub range: Range<usize>,
    pub end: Option<Range<usize>>,
}

/// 在指定光标位置按 Enter 后，目标行应采用的缩进。
///
/// `base_indent` 从最近的代码行继承，`additional_levels` 则由语言的 Tree-sitter 缩进查询决定。编辑器只需按其自身的 Tab 配置将额外层级转为空白字符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewlineIndent {
    pub base_indent: String,
    pub additional_levels: usize,
}

/// 一个可折叠范围。
///
/// 范围 = [入口行行尾换行符, 闭合括号前)：入口行全文与闭合括号保留可见，折叠后占位符与闭合尾段拼在同一显示行。
/// 无括号对的折叠（注释组等）终点在末隐藏行内容末尾。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldRange {
    pub range: Range<usize>,
}

/// 文件级语法大纲项。
///
/// 所有范围都是当前 `SyntaxSnapshot` 对应文本中的 UTF-8 字节范围，且为右开区间。
/// 该类型不携带显示行、UI 文案或可写状态；组合文档和编辑器在各自边界完成坐标映射。
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
    /// Tree-sitter 定义节点种类。
    pub kind: String,
    /// 名称前的语法上下文，例如 `pub fn` 或 `class`。
    pub context: Option<String>,
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

/// 光标或选区对应的语法节点摘要。
///
/// 节点范围使用源文本 UTF-8 字节坐标。
/// 摘要不持有 tree-sitter 的节点引用，因而可以安全地跨越查询调用边界；下一次编辑后必须重新查询。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxNode {
    /// 产生结果的文本版本。
    pub version: BufferVersion,
    /// 节点在源文本中的右开范围。
    pub range: Range<usize>,
    /// Tree-sitter 节点种类。
    pub kind: String,
    /// 产生该节点的语法层名称。
    pub language: &'static str,
    /// 产生该节点的注入层深度，主语言层为 0。
    pub language_depth: u32,
    /// 是否为命名节点；匿名标点和操作符节点为 false。
    pub is_named: bool,
    /// 是否为语法错误节点。
    pub is_error: bool,
    /// 是否为缺失节点。
    pub is_missing: bool,
}

impl SyntaxNode {
    fn from_tree_node(
        node: tree_sitter::Node<'_>,
        version: BufferVersion,
        language: &'static str,
        language_depth: u32,
    ) -> Self {
        Self {
            version,
            range: node.byte_range(),
            kind: node.kind().to_owned(),
            language,
            language_depth,
            is_named: node.is_named(),
            is_error: node.is_error(),
            is_missing: node.is_missing(),
        }
    }
}

#[derive(Clone, Debug)]
struct OutlineCandidate {
    item: OutlineItem,
}

impl SyntaxSnapshot {
    /// 返回光标所在语法层的最深节点。
    ///
    /// 注入层优先于宿主层；
    /// 空白属于 Tree-sitter 的 extras，通常没有独立节点，因此会落到包含它的语法根节点。
    /// 文件末尾光标按前一个 UTF-8 字节处理，保证闭合节点后的末尾光标仍能进行结构扩展。
    pub fn node_at(&self, offset: usize, text: &Snapshot) -> Option<SyntaxNode> {
        let ancestors = self.node_ancestors(offset..offset, text);
        ancestors.into_iter().next()
    }

    /// 返回选区所在语法层的节点链，顺序为最小节点到语法根节点。
    ///
    /// 只选择一个最深语法层，结构化选择不会从注入代码跨回宿主文档。
    pub fn node_ancestors(&self, range: Range<usize>, text: &Snapshot) -> Vec<SyntaxNode> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }

        let query_range = if range.start == range.end && range.start == text.len_bytes().get() {
            range.start.saturating_sub(1)..range.end
        } else {
            range.clone()
        };
        let mut best: Option<(u32, Vec<SyntaxNode>)> = None;
        for layer in self.layers_for_range(&query_range) {
            let Some(mut node) = layer
                .tree
                .root_node()
                .descendant_for_byte_range(query_range.start, query_range.end)
            else {
                continue;
            };

            while !encloses(&node.byte_range(), &range) {
                let Some(parent) = node.parent() else {
                    break;
                };
                node = parent;
            }
            if !encloses(&node.byte_range(), &range) {
                continue;
            }

            let mut nodes = Vec::new();
            let mut current = Some(node);
            while let Some(node) = current {
                nodes.push(SyntaxNode::from_tree_node(
                    node,
                    self.version,
                    layer.language.name(),
                    layer.depth,
                ));
                current = node.parent();
            }

            let replace = best.as_ref().is_none_or(|(depth, current)| {
                layer.depth > *depth
                    || (layer.depth == *depth
                        && nodes.first().is_some_and(|node| {
                            current
                                .first()
                                .is_none_or(|current| node.range.len() < current.range.len())
                        }))
            });
            if replace {
                best = Some((layer.depth, nodes));
            }
        }
        best.map(|(_, nodes)| nodes).unwrap_or_default()
    }

    /// 将选区扩展到当前语法层中严格包围它的下一个节点。
    pub fn expand_selection_range(
        &self,
        range: Range<usize>,
        text: &Snapshot,
    ) -> Option<Range<usize>> {
        self.node_ancestors(range.clone(), text)
            .into_iter()
            .map(|node| node.range)
            .find(|candidate| candidate.len() > range.len())
    }

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
                    let context = context_text(text, &contexts, &item_range);
                    let body_range = paired_body_range(&opens, &closes, &item_range);
                    candidates.push(OutlineCandidate {
                        item: OutlineItem {
                            version: self.version,
                            range: item_range,
                            name_range,
                            name,
                            kind: item_node.kind().to_owned(),
                            context,
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

    /// `outline` 的语义别名，供文件符号消费方使用更明确的名称。
    pub fn outline_items(&self, range: Range<usize>, text: &Snapshot) -> Vec<OutlineItem> {
        self.outline(range, text)
    }

    pub fn bracket_pairs(&self, range: Range<usize>, text: &Snapshot) -> Vec<BracketPair> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        let mut pairs = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.brackets() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some(query_match) = matches.next() {
                let mut open = None;
                let mut close = None;
                for capture in query_match.captures {
                    match names.get(capture.index as usize).copied() {
                        Some("open") => open = Some(capture.node.byte_range()),
                        Some("close") => close = Some(capture.node.byte_range()),
                        _ => {}
                    }
                }
                if let (Some(open), Some(close)) = (open, close) {
                    pairs.push(BracketPair { open, close });
                }
            }
        }
        pairs.sort_unstable_by_key(|pair| (pair.open.start, pair.close.end));
        pairs
    }

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
    /// 这与编辑器 UI 无关：语言层负责找到缩进基准和未闭合的语法结构，编辑器负责将结果应用到插入文本。
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

    pub fn fold_ranges(&self, range: Range<usize>, text: &Snapshot) -> Vec<FoldRange> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        // 折叠查询的约定：@fold 指定语法区域；需要保留闭合符号的区域以 @fold.end 声明其边界。
        //
        // 区域本身以定界符开头时，直接使用同起点的括号对；否则折叠到区域末尾。
        // 不能从区域内部猜测闭合符号：Markdown section、跨行函数签名等结构性区域都可能包含独立的配对节点。
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

                // 同一个 match 命中多个节点时（如 `+` 组捕获的连续注释），行相邻则合并成一个折叠范围。
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
                    // 结构性区域（section、函数定义、注释组等）没有自身闭合符号，终点 = 末隐藏行内容末尾。
                    //
                    // line_comment 等节点含尾随换行（end 落在下一行行首），按"结束恰在行首则回退一行"换算真实末行。
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
            // 单行范围没有可隐藏的行，折叠无意义。
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

fn context_text(
    text: &Snapshot,
    nodes: &[tree_sitter::Node<'_>],
    item_range: &Range<usize>,
) -> Option<String> {
    let mut ranges: Vec<_> = nodes
        .iter()
        .map(|node| node.byte_range())
        .filter(|range| encloses(item_range, range))
        .collect();
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    let mut context = String::new();
    for range in ranges {
        let Some(value) = node_text(text, range) else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        if !context.is_empty() {
            context.push(' ');
        }
        context.push_str(&value);
    }
    (!context.is_empty()).then_some(context)
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
    // 属性/装饰器通常与定义只隔一个换行；连续注释也通过相邻范围合并。
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

/// 行终止换行符（`\n`）的字节位置；行尾无换行符时返回行尾。
///
/// 折叠范围从该位置开始：入口行换行符被折叠吞掉，占位符与闭合尾段拼在同一显示行。
fn line_newline_position(text: &Snapshot, line: Line) -> ByteOffset {
    let content = text
        .line_content(line, None)
        .expect("折叠入口行必须位于当前 Snapshot 内");
    if content.text_range().end() == content.full_range().end() {
        content.full_range().end()
    } else {
        // 行含终止换行符：`\r?\n` 的 `\n` 位于行尾前一字节。
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

#[cfg(test)]
mod tests {
    use super::NewlineIndent;
    use crate::test::{parsed_syntax, rust_buffer};
    use zcv_text::ByteOffset;

    #[test]
    fn rust_syntax_snapshot_exposes_zed_structure_queries() {
        let source = "struct Demo { value: i32 }\nfn main() { let x = (1 + 2); }\n";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let full = 0..snapshot.len_bytes().get();

        let brackets = syntax.bracket_pairs(full.clone(), &snapshot);
        assert!(brackets.iter().any(|pair| {
            &source[pair.open.clone()] == "(" && &source[pair.close.clone()] == ")"
        }));

        assert!(!syntax.indent_ranges(full, &snapshot).is_empty());
    }

    #[test]
    fn outline_preserves_nested_same_named_unicode_definitions() {
        let source = "mod 数据 {\n    struct Item {\n        value: i32,\n    }\n    fn build() {\n        let value = 1;\n    }\n}\nfn build() {}\n";
        let (buffer, syntax) = parsed_syntax("outline.rs", source);
        let snapshot = buffer.snapshot();
        let item_start = source.find("Item").unwrap();
        let items = syntax
            .snapshot()
            .outline(0..snapshot.len_bytes().get(), &snapshot);

        let names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
        assert!(names.contains(&"数据"));
        assert!(names.iter().filter(|name| **name == "build").count() == 2);
        assert!(items.iter().any(|item| {
            item.name == "Item"
                && item.depth > 0
                && item.name_range == (item_start..item_start + "Item".len())
        }));
        assert!(items.iter().all(|item| item.version == snapshot.version()));
    }

    #[test]
    fn outline_covers_markdown_and_html_injection_layers_in_source_coordinates() {
        let markdown = "# 文档\n\n```rust\nfn 初始化() {}\n```\n\n## 子节\n";
        let (buffer, syntax) = parsed_syntax("README.md", markdown);
        let snapshot = buffer.snapshot();
        let items = syntax
            .snapshot()
            .outline(0..snapshot.len_bytes().get(), &snapshot);
        assert!(
            items
                .iter()
                .any(|item| item.name == "文档" && item.language == "Markdown")
        );
        let function_start = markdown.find("初始化").unwrap();
        let function = items
            .iter()
            .find(|item| item.name == "初始化")
            .expect("围栏内 Rust 函数应出现在大纲中");
        assert_eq!(function.language, "Rust");
        assert_eq!(function.name_range.start, function_start);
        assert!(function.depth > 0);

        let html = "<main><section><h1>标题</h1></section></main>";
        let (buffer, syntax) = parsed_syntax("index.html", html);
        let snapshot = buffer.snapshot();
        let items = syntax
            .snapshot()
            .outline(0..snapshot.len_bytes().get(), &snapshot);
        let section = items
            .iter()
            .find(|item| item.name == "section")
            .expect("HTML 元素应出现在大纲中");
        assert_eq!(&html[section.name_range.clone()], "section");
        assert!(
            items
                .iter()
                .any(|item| item.name == "h1" && item.depth > section.depth)
        );
    }

    #[test]
    fn outline_is_empty_without_declared_query() {
        let (buffer, syntax) = parsed_syntax("notes.txt", "标题\n");
        let snapshot = buffer.snapshot();
        assert!(
            syntax
                .snapshot()
                .outline(0..snapshot.len_bytes().get(), &snapshot)
                .is_empty()
        );
    }

    #[test]
    fn syntax_nodes_use_utf8_ranges_and_expand_within_one_layer() {
        let source = "fn 数据() {\n    let 值 = (1 + 2);\n}\n";
        let (buffer, syntax) = parsed_syntax("nodes.rs", source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let name_start = source.find("值").unwrap();
        let node = syntax
            .node_at(name_start, &snapshot)
            .expect("Unicode 标识符应有语法节点");

        assert_eq!(&source[node.range.clone()], "值");
        assert_eq!(node.kind, "identifier");
        assert_eq!(node.language, "Rust");
        assert!(node.is_named);
        assert_eq!(node.version, snapshot.version());

        let ancestors = syntax.node_ancestors(name_start..name_start, &snapshot);
        assert_eq!(ancestors.first().map(|node| &node.range), Some(&node.range));
        assert!(ancestors.iter().any(|node| node.kind == "function_item"));

        let expanded = syntax
            .expand_selection_range(name_start..name_start, &snapshot)
            .expect("光标应能先扩展到标识符");
        assert_eq!(&source[expanded], "值");

        let anonymous = syntax
            .node_at(source.find('(').unwrap(), &snapshot)
            .expect("匿名括号节点应可导航");
        assert_eq!(anonymous.kind, "(");
        assert!(!anonymous.is_named);
        let whitespace_offset = source.find('\n').unwrap();
        let whitespace = syntax
            .node_at(whitespace_offset, &snapshot)
            .expect("空白位置应稳定落到包含它的语法根节点");
        assert!(whitespace.range.start <= whitespace_offset);
        assert!(whitespace_offset <= whitespace.range.end);
        assert!(syntax.node_at(source.len(), &snapshot).is_some());
    }

    #[test]
    fn syntax_nodes_prefer_injection_layer_and_do_not_cross_back_to_host() {
        let source = "# 文档\n\n```rust\nfn 初始化() {}\n```\n";
        let (buffer, syntax) = parsed_syntax("README.md", source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let name_start = source.find("初始化").unwrap();
        let node = syntax
            .node_at(name_start, &snapshot)
            .expect("围栏内函数名应有语法节点");

        assert_eq!(node.language, "Rust");
        assert_eq!(&source[node.range.clone()], "初始化");
        assert!(
            syntax
                .node_ancestors(name_start..name_start, &snapshot)
                .iter()
                .all(|node| node.language == "Rust")
        );
    }

    #[test]
    fn syntax_nodes_keep_error_nodes_visible_for_incomplete_input() {
        let source = "fn main( {\n";
        let (buffer, syntax) = parsed_syntax("broken.rs", source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let brace = source.find('{').unwrap();
        let ancestors = syntax.node_ancestors(brace..brace, &snapshot);

        assert!(
            ancestors.iter().any(|node| node.is_error),
            "不完整 Rust 输入的祖先链必须保留 ERROR 节点"
        );
    }

    #[test]
    fn syntax_selection_reaches_file_root_for_import_and_structures() {
        let source = "use gpui::{\n    AnyElement,\n    AnyView,\n    App,\n};\n\nstruct EditorState {\n    value: usize,\n}\n";
        let (buffer, syntax) = parsed_syntax("selection.rs", source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();

        for needle in ["AnyElement", "EditorState"] {
            let mut range = source.find(needle).unwrap()..source.find(needle).unwrap();
            for _ in 0..16 {
                let Some(next) = syntax.expand_selection_range(range.clone(), &snapshot) else {
                    break;
                };
                range = next;
            }
            assert_eq!(range, 0..source.len(), "{needle} 应能扩展到文件根节点");
        }
    }

    #[test]
    fn baseline_languages_expose_brackets_indents_and_folds() {
        let cases = [
            ("main.c", "int main() {\n  return 0;\n}\n"),
            (
                "main.cpp",
                "class Greeter {\npublic:\n  void greet() {}\n};\n",
            ),
            (
                "Program.cs",
                "class Program {\n  static void Main() {}\n}\n",
            ),
            ("main.go", "package main\nfunc main() {\n  println(1)\n}\n"),
            (
                "app.rb",
                "class Greeter\n  def greet(name)\n    name\n  end\nend\n",
            ),
            (
                "index.php",
                "<?php\nfunction greet($name) {\n  return $name;\n}\n",
            ),
            (
                "main.swift",
                "struct Greeter {\n  func greet() {\n    print(1)\n  }\n}\n",
            ),
            (
                "Main.kt",
                "class Greeter {\n  fun greet() {\n    println(1)\n  }\n}\n",
            ),
            (
                "init.lua",
                "local function greet(name)\n  return name\nend\n",
            ),
            ("main.zig", "pub fn main() void {\n  const value = 1;\n}\n"),
            (
                "query.sql",
                "SELECT name\nFROM (\n  SELECT name FROM users\n) nested;\n",
            ),
        ];

        for (path, source) in cases {
            let (buffer, syntax) = parsed_syntax(path, source);
            let snapshot = buffer.snapshot();
            let syntax = syntax.snapshot();
            let full = 0..snapshot.len_bytes().get();
            assert!(
                !syntax.bracket_pairs(full.clone(), &snapshot).is_empty(),
                "{path} 应产生括号配对"
            );
            assert!(
                !syntax.indent_ranges(full.clone(), &snapshot).is_empty(),
                "{path} 应产生缩进范围"
            );
            assert!(
                !syntax.fold_ranges(full, &snapshot).is_empty(),
                "{path} 应产生折叠范围"
            );
        }
    }

    #[test]
    fn existing_languages_with_new_fold_queries_produce_ranges() {
        let cases = [
            ("main.py", "def greet():\n    return 1\n"),
            ("main.js", "function greet() {\n  return 1;\n}\n"),
            ("Main.java", "class Main {\n  static void main() {}\n}\n"),
            ("script.sh", "function greet() {\n  echo hi\n}\n"),
            ("Cargo.toml", "[package]\nname = \"zcv\"\nversion = \"1\"\n"),
            ("data.json", "{\n  \"name\": \"zcv\"\n}\n"),
            ("data.yaml", "root:\n  child:\n    value: 1\n"),
            ("README.md", "# 标题\n\n第一段。\n\n第二段。\n"),
            ("index.html", "<main>\n  <p>text</p>\n</main>\n"),
            ("style.css", ".card {\n  color: red;\n}\n"),
        ];

        for (path, source) in cases {
            let (buffer, syntax) = parsed_syntax(path, source);
            let snapshot = buffer.snapshot();
            let folds = syntax
                .snapshot()
                .fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
            assert!(!folds.is_empty(), "{path} 应产生折叠范围");
        }
    }

    #[test]
    fn markdown_section_folds_through_nested_fenced_code() {
        let source = "\
# 第一节

正文。

```rust
let value = 1;
```

标题后的正文。

# 第二节

不应属于第一节。
";
        let (buffer, syntax) = parsed_syntax("README.md", source);
        let snapshot = buffer.snapshot();
        let folds = syntax
            .snapshot()
            .fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
        let first_section = folds
            .iter()
            .find(|fold| fold.range.start == source.find('\n').unwrap())
            .expect("第一节应产生折叠范围");

        assert!(
            source[first_section.range.clone()].contains("标题后的正文。"),
            "标题折叠必须覆盖 section 的全部正文，而不是在内部代码围栏前截断"
        );
        assert!(
            !source[first_section.range.clone()].contains("# 第二节"),
            "标题折叠不得吞入同级标题"
        );
    }

    #[test]
    fn structural_fold_ignores_nested_multiline_delimiters() {
        let source = "\
def build(
    first,
    second,
):
    return first + second
";
        let (buffer, syntax) = parsed_syntax("build.py", source);
        let snapshot = buffer.snapshot();
        let folds = syntax
            .snapshot()
            .fold_ranges(0..snapshot.len_bytes().get(), &snapshot);

        assert!(
            folds
                .iter()
                .any(|fold| source[fold.range.clone()].contains("return first + second")),
            "函数折叠必须覆盖函数体，不能在跨行参数列表的右括号前截断"
        );
    }

    #[test]
    fn macro_definition_declares_its_closing_boundary() {
        let source = "\
macro_rules! pair {
    ($value:expr) => {
        ($value, $value)
    };
}
";
        let (buffer, syntax) = parsed_syntax("macros.rs", source);
        let snapshot = buffer.snapshot();
        let folds = syntax
            .snapshot()
            .fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
        let outer = folds
            .iter()
            .find(|fold| fold.range.start == source.find('\n').unwrap())
            .expect("宏定义应产生折叠范围");

        assert_eq!(
            outer.range.end,
            source.rfind('}').unwrap(),
            "宏定义的外层闭合花括号必须保留可见"
        );
    }

    #[test]
    fn rust_newline_indent_is_computed_in_the_language_layer() {
        let source = "fn main() {\n    build()\n}";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let after_open_paren = source.find("build(").unwrap() + "build(".len();
        let after_closed_call = source.find("build()").unwrap() + "build()".len();

        assert_eq!(
            syntax
                .suggested_newline_indent(ByteOffset::new(after_open_paren), &snapshot)
                .unwrap(),
            NewlineIndent {
                base_indent: "    ".to_owned(),
                additional_levels: 1,
            }
        );
        assert_eq!(
            syntax
                .suggested_newline_indent(ByteOffset::new(after_closed_call), &snapshot)
                .unwrap(),
            NewlineIndent {
                base_indent: "    ".to_owned(),
                additional_levels: 0,
            }
        );
    }

    #[test]
    fn rust_fold_ranges_cover_blocks_and_skip_single_lines() {
        let source = "\
struct Demo {
    value: i32,
}

impl Demo {
    fn new() -> Self {
        // 单行注释不产生折叠。
        let value = 1;
        // 连续注释折叠为一个组。
        // 第二行注释。
        Self { value }
    }
}

fn main() {
    let x = 1;
}
";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let folds = syntax.fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
        let texts: Vec<&str> = folds
            .iter()
            .map(|fold| &source[fold.range.clone()])
            .collect();

        // 各块体（field_declaration_list / declaration_list / block）都被覆盖，
        // 范围 = [入口行换行符, 闭合括号前)：入口行与闭合括号不在范围内。
        assert!(texts.contains(&"\n    value: i32,\n"));
        assert!(texts.contains(&"\n    fn new() -> Self {\n        // 单行注释不产生折叠。\n        let value = 1;\n        // 连续注释折叠为一个组。\n        // 第二行注释。\n        Self { value }\n    }\n"));
        assert!(texts.contains(&"\n        // 单行注释不产生折叠。\n        let value = 1;\n        // 连续注释折叠为一个组。\n        // 第二行注释。\n        Self { value }\n    "));
        // 连续注释组折叠为一个范围（入口行保留，其余行隐藏）。
        assert!(texts.contains(&"\n        // 第二行注释。"));

        // 嵌套结构：外层范围完整包含内层范围。
        let outer = folds
            .iter()
            .find(|fold| {
                &source[fold.range.clone()]
                    == "\n    fn new() -> Self {\n        // 单行注释不产生折叠。\n        let value = 1;\n        // 连续注释折叠为一个组。\n        // 第二行注释。\n        Self { value }\n    }\n"
            })
            .unwrap();
        let inner = folds
            .iter()
            .find(|fold| {
                fold.range.start >= outer.range.start
                    && fold.range.end <= outer.range.end
                    && fold.range != outer.range
            })
            .expect("impl 块内应存在嵌套折叠范围");

        assert!(inner.range.start > outer.range.start && inner.range.end < outer.range.end);
    }

    #[test]
    fn use_declarations_fold_independently_and_skip_single_lines() {
        let source = "\
use std::collections::BTreeMap;
use std::ops::{
    Range,
    Deref,
};
use std::sync::Arc;

use zcv_text::{
    Buffer,
    Snapshot,
};
";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let folds = syntax.fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
        let texts: Vec<&str> = folds
            .iter()
            .map(|fold| &source[fold.range.clone()])
            .collect();

        // 两个多行 use 各自独立成折叠范围（入口行与闭合括号 `}` 保留，尾段 `;` 可见）。
        assert!(texts.contains(&"\n    Range,\n    Deref,\n"));
        assert!(texts.contains(&"\n    Buffer,\n    Snapshot,\n"));
        // 单行 use 不产生折叠。
        assert!(!texts.contains(&"use std::collections::BTreeMap;"));
    }

    #[test]
    fn single_line_doc_comments_do_not_fold() {
        // 回归：tree-sitter-rust 的 line_comment 节点含尾随换行（end 落在下一行行首），
        // 单行过滤必须用 buffer 行语义，否则单行注释会误判为可折叠。
        let source = "\
/// Editor 自身的领域事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    /// 编辑器关联的文件路径发生变化。
    PathChanged,
}
";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let folds = syntax.fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
        let texts: Vec<&str> = folds
            .iter()
            .map(|fold| &source[fold.range.clone()])
            .collect();

        // 只有 enum 块体可折叠；单行 doc 注释不产生折叠。
        assert!(texts.contains(&"\n    /// 编辑器关联的文件路径发生变化。\n    PathChanged,\n"));
        assert!(!texts.iter().any(|text| text.starts_with("///")));
    }

    #[test]
    fn multi_line_macro_invocation_folds_but_single_line_does_not() {
        let source = "\
fn main() {
    let x = vec![
        1,
        2,
    ];
    println!(\"ok\");
    actions!(
        editor,
        [
            MoveLeft,
            MoveRight,
        ],
    );
    let y = format!(\"{}: {}\", 1, 2);
}
";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let folds = syntax.fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
        let texts: Vec<&str> = folds
            .iter()
            .map(|fold| &source[fold.range.clone()])
            .collect();

        // 跨行宏调用整体成折叠范围（入口行保留，闭合括号前收口：`vec![...]` 收在 `]` 前）。
        assert!(texts.contains(&"\n        1,\n        2,\n    "));
        assert!(texts.contains(&"\n        editor,\n        [\n            MoveLeft,\n            MoveRight,\n        ],\n    "));
        // 单行宏调用不产生折叠。
        assert!(!texts.contains(&"println!(\"ok\")"));
        assert!(!texts.contains(&"format!(\"{}: {}\", 1, 2)"));
    }
}
