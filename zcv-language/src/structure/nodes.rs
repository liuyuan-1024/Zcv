//! 语法节点查询。
//!
//! 这里把 Tree-sitter 节点转换成不持有树引用的摘要，因此结果可以离开查询过程使用。
//! 节点范围始终是当前文本的 UTF-8 字节右开区间；
//! 注入语言的坐标沿用外层源文本坐标。

use std::ops::Range;

use zcv_text::Snapshot;

use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::encloses;

/// 光标或选区对应的语法节点摘要。
///
/// 摘要不持有 Tree-sitter 的节点引用；
/// 下一次编辑后必须重新查询。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxNode {
    /// 产生结果的文本版本。
    pub version: zcv_text::BufferVersion,
    /// 节点在源文本中的右开范围。
    pub range: Range<usize>,
    /// Tree-sitter 节点种类。
    pub kind: String,
    /// 产生结果的语法层名称。
    pub language: &'static str,
    /// 产生结果的注入层深度，主语言层为 0。
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
        version: zcv_text::BufferVersion,
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

impl SyntaxSnapshot {
    /// 返回光标所在语法层的最深节点。
    pub fn node_at(&self, offset: usize, text: &Snapshot) -> Option<SyntaxNode> {
        self.node_ancestors(offset..offset, text).into_iter().next()
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
}
