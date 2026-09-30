//! Editor 对语法智能结果的交互门面。
//!
//! Tree-sitter 查询由 `zcv-language` 负责，`DisplayMap` 负责把组合文档坐标投影到查询结果。
//! 本模块只把这些结果转换为编辑器级的筛选、导航和编辑操作。

use std::ops::Range;

use super::{Editor, NAVIGATION_TOP_OFFSET};
use gpui::{App, Context, HighlightStyle};
use zcv_language::{LocalBinding, OutlineItem};
use zcv_multi_buffer::MultiBufferSnapshot;
use zcv_text::BufferVersion;

/// 大纲失效键：组合文本/拓扑版本与源元数据版本。
///
/// 只有两者都相同时已有大纲才可复用。滚动、绘制与选择变化不改变它们；
/// 编辑、excerpt/diff 拓扑变化与语法重解析会推进其中至少一个。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutlineVersion {
    text: BufferVersion,
    metadata: u64,
}

impl OutlineVersion {
    fn of(snapshot: &MultiBufferSnapshot) -> Self {
        Self {
            text: snapshot.version(),
            metadata: snapshot.metadata_version(),
        }
    }
}

/// 大纲派生输入：某一确认版本上的组合快照。
///
/// 供大纲等派生消费方在后台计算，并在安装前用 Editor::outline_version 校验；
/// 消费方不直接持有或改写组合文档。
#[derive(Clone)]
pub struct OutlineSource {
    snapshot: MultiBufferSnapshot,
}

impl OutlineSource {
    /// 该输入对应的失效键。
    pub fn version(&self) -> OutlineVersion {
        OutlineVersion::of(&self.snapshot)
    }

    /// 在当前版本上计算大纲项；不访问界面状态，可在后台线程调用。
    pub fn items(&self) -> Vec<OutlineItem> {
        self.snapshot.outline_items()
    }
}

impl Editor {
    /// 返回当前组合文档的文件级语法大纲。
    pub fn outline_items(&self, cx: &App) -> Vec<OutlineItem> {
        self.display_snapshot(cx).buffer_snapshot().outline_items()
    }

    /// 当前大纲失效键；O(1)，不触发语法查询。
    pub fn outline_version(&self, cx: &App) -> OutlineVersion {
        OutlineVersion::of(self.display_snapshot(cx).buffer_snapshot())
    }

    /// 大纲派生输入；消费方在后台计算后按版本安装。
    pub fn outline_source(&self, cx: &App) -> OutlineSource {
        OutlineSource {
            snapshot: self.display_snapshot(cx).buffer_snapshot().clone(),
        }
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
                .display_snapshot(cx)
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
    pub fn local_bindings(&self, cx: &App) -> Vec<LocalBinding> {
        self.display_snapshot(cx).buffer_snapshot().local_bindings()
    }

    /// 将大纲项定位到其名称范围，并拒绝异步刷新后已经失效的结果。
    pub fn navigate_to_outline_item(&mut self, item: &OutlineItem, cx: &mut Context<Self>) -> bool {
        let current = self.outline_items(cx).into_iter().any(|current| {
            current.version == item.version
                && current.range == item.range
                && current.name_range == item.name_range
                && current.name == item.name
        });
        if !current {
            return false;
        }
        self.select_byte_range(item.name_range.clone(), cx);
        self.request_scroll_to_top(NAVIGATION_TOP_OFFSET, cx);
        true
    }
}
