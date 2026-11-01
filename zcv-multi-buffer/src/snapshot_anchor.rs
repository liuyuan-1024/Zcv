//! `MultiBufferSnapshot` 将外部 `Anchor` 解析为组合偏移。
//!
//! 本文件的类型与字段仍定义在 crate 根模块 `multi_buffer.rs`。

use super::*;

impl MultiBufferSnapshot {
    /// 把快照内的组合偏移锚定到底层源坐标（Editor 源锚点选区：投影→源）。
    pub fn anchor_at(
        &self,
        offset: impl Into<MultiBufferOffset>,
        affinity: Affinity,
    ) -> MultiBufferAnchor {
        let offset: MultiBufferOffset = offset.into();
        anchor_in_mappings(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
            offset.into(),
            affinity,
        )
        .unwrap_or_else(|| MultiBufferAnchor::boundary(offset))
    }

    /// 把稳定锚点定位到当前组合坐标。
    ///
    /// 这是位置状态（选择、滚动）的总解析：
    /// 源或 excerpt 退出当前投影时，按路径顺序定位到重建后结构中的相邻边界；空投影则落在文首。
    /// 源 Anchor 版本无法推进到当前源快照仍是版本链错误，必须显式处理。
    pub fn anchor_offset(&self, anchor: &MultiBufferAnchor) -> TextResult<MultiBufferOffset> {
        resolve_anchor_in_mappings(
            &self.excerpts,
            &self.diff_transforms,
            &self.path_keys,
            &self.excerpt_sources,
            &self.source_indices,
            anchor,
        )
        .map(|resolution| resolution.offset().into())
    }

    /// 只在锚点仍属于当前可见源片段时返回组合坐标。
    ///
    /// 折叠、搜索命中和自动闭合等附属状态不能在源退出投影后迁移到相邻文件，因此与位置状态使用不同的解析语义。
    pub fn projected_anchor_offset(
        &self,
        anchor: &MultiBufferAnchor,
    ) -> TextResult<Option<MultiBufferOffset>> {
        resolve_anchor_in_mappings(
            &self.excerpts,
            &self.diff_transforms,
            &self.path_keys,
            &self.excerpt_sources,
            &self.source_indices,
            anchor,
        )
        .map(|resolution| resolution.projected_offset().map(Into::into))
    }
}
