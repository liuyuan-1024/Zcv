//! Editor 对语法智能结果的交互门面。
//!
//! Tree-sitter 查询由 `zcv-language` 负责，`DisplayMap` 负责把组合文档坐标投影到查询结果。
//! 本模块只把这些结果转换为编辑器级的筛选、导航和编辑操作。

use std::ops::Range;

use gpui::{Context, HighlightStyle};
use zcv_language::{LocalBinding, OutlineItem, SyntaxNode};
use zcv_text::ByteOffset;

use super::{Editor, NAVIGATION_TOP_OFFSET, edit_metadata};
use crate::selection::{Selection, SelectionSet, replace_selections};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LocalRenameError {
    #[error("当前文档不是可安全重命名的单文件文档")]
    UnsupportedDocument,
    #[error("新名称不能为空或不是有效标识符")]
    InvalidName,
    #[error("当前位置没有可确定的局部绑定")]
    BindingNotFound,
    #[error("局部重命名事务失败：{0}")]
    Edit(String),
}

impl Editor {
    /// 返回当前组合文档的文件级语法大纲。
    pub fn outline_items(&self) -> Vec<OutlineItem> {
        self.display_map.outline_items()
    }

    /// 按大纲文本过滤当前文件大纲；匹配不改变语法层结果的顺序和层级。
    pub fn outline_items_matching(&self, query: &str) -> Vec<OutlineItem> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return self.outline_items();
        }
        self.outline_items()
            .into_iter()
            .filter(|item| item.text.to_lowercase().contains(&query))
            .collect()
    }

    /// 返回大纲标签的已解析语法样式。
    ///
    /// 语法查询和 capture 到主题样式的映射均由 DisplayMap 完成，大纲面板只负责展示结果。
    pub fn outline_item_highlights(
        &self,
        item: &OutlineItem,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let mut highlights = Vec::new();
        for part in &item.text_ranges {
            let source_start = part.source_range.start;
            for (range, style) in self
                .display_map
                .highlights_for_range(part.source_range.clone())
            {
                let start = range.start.max(part.source_range.start);
                let end = range.end.min(part.source_range.end);
                if start < end {
                    highlights.push((
                        (part.text_range.start + start - source_start)
                            ..(part.text_range.start + end - source_start),
                        style,
                    ));
                }
            }
        }
        highlights
    }

    /// 返回当前单文件文档中可确定归属的局部绑定。
    pub fn local_bindings(&self) -> Vec<LocalBinding> {
        self.display_map.local_bindings()
    }

    /// 在当前光标位置安全重命名局部绑定。
    ///
    /// 只有语法层明确解析出的定义和引用会参与编辑；
    /// 组合文档、过期快照和未解析名称均拒绝操作。
    pub fn rename_local_at(
        &mut self,
        offset: ByteOffset,
        new_name: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalRenameError> {
        if self.is_read_only(cx) {
            return Err(LocalRenameError::UnsupportedDocument);
        }
        if !is_valid_identifier(new_name) {
            return Err(LocalRenameError::InvalidName);
        }

        let binding = self
            .local_bindings()
            .into_iter()
            .find(|binding| binding_contains(binding, offset.get()))
            .ok_or(LocalRenameError::BindingNotFound)?;

        let mut ranges = Vec::with_capacity(binding.references.len() + 1);
        ranges.push(binding.definition_range);
        ranges.extend(binding.references);
        ranges.sort_unstable_by_key(|range| (range.start, range.end));
        ranges.dedup();
        let targets = SelectionSet::new(
            ranges
                .into_iter()
                .map(|range| {
                    Selection::new(ByteOffset::new(range.start), ByteOffset::new(range.end))
                })
                .collect(),
        );
        let before_selections = self.resolved_selections();
        let metadata = edit_metadata("重命名局部绑定");
        let replacement = new_name.to_owned();
        self.change_with_after(before_selections, metadata.clone(), cx, move |buffer| {
            replace_selections(buffer, &targets, &replacement, metadata)
        })
        .map(|_| ())
        .map_err(|error| LocalRenameError::Edit(error.to_string()))
    }

    /// 返回组合文档中指定光标的语法节点；结果与当前 Editor 快照版本绑定。
    pub fn syntax_node_at(&self, offset: ByteOffset) -> Option<SyntaxNode> {
        self.display_map.syntax_node_at(offset)
    }

    /// 返回指定选区的语法祖先链，顺序为最小节点到语法根节点。
    pub fn syntax_node_ancestors(&self, range: Range<usize>) -> Vec<SyntaxNode> {
        self.display_map.syntax_node_ancestors(range)
    }

    /// 将大纲项定位到其名称范围，并拒绝异步刷新后已经失效的结果。
    pub fn navigate_to_outline_item(&mut self, item: &OutlineItem, cx: &mut Context<Self>) -> bool {
        let current = self.outline_items().into_iter().any(|current| {
            current.version == item.version
                && current.range == item.range
                && current.name_range == item.name_range
                && current.name == item.name
        });
        if !current {
            return false;
        }
        self.select_byte_range(item.name_range.clone(), cx);
        self.request_scroll_to_top(NAVIGATION_TOP_OFFSET);
        true
    }
}

fn binding_contains(binding: &LocalBinding, offset: usize) -> bool {
    std::iter::once(&binding.definition_range)
        .chain(binding.references.iter())
        .any(|range| range.start <= offset && offset <= range.end)
}

fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_alphabetic())
        && chars.all(|character| character == '_' || character.is_alphanumeric())
}
