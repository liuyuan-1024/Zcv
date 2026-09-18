//! Editor 对语法智能结果的交互门面。
//!
//! Tree-sitter 查询由 `zcv-language` 负责，`DisplayMap` 负责把组合文档坐标投影到查询结果。
//! 本模块只把这些结果转换为编辑器级的筛选、导航和编辑操作。

use std::ops::Range;

use super::{Editor, NAVIGATION_TOP_OFFSET};
use gpui::{App, Context, HighlightStyle};
use zcv_language::{LocalBinding, OutlineItem};

impl Editor {
    /// 返回当前组合文档的文件级语法大纲。
    pub fn outline_items(&self) -> Vec<OutlineItem> {
        self.snapshot.buffer_snapshot().outline_items()
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
        cx: &App,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let mut highlights = Vec::new();
        for part in &item.text_ranges {
            let source_start = part.source_range.start;
            for (range, style) in self
                .snapshot
                .display_snapshot
                .highlights_for_range(part.source_range.clone(), cx)
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
        self.snapshot.buffer_snapshot().local_bindings()
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
