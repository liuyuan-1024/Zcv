//! Editor 对语法智能结果的交互门面。
//!
//! Tree-sitter 查询由 `zcv-language` 负责，`DisplayMap` 负责把组合文档坐标投影到查询结果。
//! 本模块只把这些结果转换为编辑器级的筛选、导航和编辑操作。

use std::ops::Range;
use std::sync::Arc;

use super::{Editor, NAVIGATION_TOP_OFFSET};
use gpui::{App, Context, HighlightStyle};
use zcv_language::{LocalBinding, OutlineItem};
use zcv_multi_buffer::{MultiBufferAnchor, MultiBufferSnapshot, OutlineEntry};
use zcv_text::BufferVersion;
use zcv_theme::syntax;

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

    /// 在当前版本上计算带文件身份的大纲条目；不访问界面状态，可在后台线程调用。
    pub fn entries(&self) -> Vec<OutlineEntry> {
        self.snapshot.outline_entries()
    }
}

impl Editor {
    /// 返回当前组合文档的文件级语法大纲。
    pub fn outline_items(&self, cx: &App) -> Vec<OutlineItem<MultiBufferAnchor>> {
        self.display_snapshot(cx).buffer_snapshot().outline_items()
    }

    /// 返回当前组合文档的大纲条目，含每条符号的文件身份。
    pub fn outline_entries(&self, cx: &App) -> Vec<OutlineEntry> {
        self.display_snapshot(cx)
            .buffer_snapshot()
            .outline_entries()
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
    /// 按当前主题解析生成时固定在标签内的 capture；不再查询当前文档。
    pub fn outline_item_highlights(
        item: &OutlineItem<MultiBufferAnchor>,
        cx: &App,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let names: Vec<Arc<str>> = item
            .highlight_ranges
            .iter()
            .map(|(_, name)| Arc::clone(name))
            .collect();
        item.highlight_ranges
            .iter()
            .map(|(range, _)| range.clone())
            .zip(syntax::style_table(&names, cx))
            .collect()
    }

    /// 返回当前单文件文档中可确定归属的局部绑定。
    pub fn local_bindings(&self, cx: &App) -> Vec<LocalBinding> {
        self.display_snapshot(cx).buffer_snapshot().local_bindings()
    }

    /// 将大纲项的稳定名称锚点解析到当前组合快照后定位。
    pub fn navigate_to_outline_item(
        &mut self,
        item: &OutlineItem<MultiBufferAnchor>,
        cx: &mut Context<Self>,
    ) -> bool {
        let snapshot = self.display_snapshot(cx);
        let buffer = snapshot.buffer_snapshot();
        let (Ok(Some(start)), Ok(Some(end))) = (
            buffer.projected_anchor_offset(&item.name_range.start),
            buffer.projected_anchor_offset(&item.name_range.end),
        ) else {
            return false;
        };
        if start > end {
            return false;
        }
        self.select_byte_range(start.get()..end.get(), cx);
        self.request_scroll_to_top(NAVIGATION_TOP_OFFSET, cx);
        true
    }
}
