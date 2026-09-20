//! SelectionSet<T>：多光标/多选区的归一化集合。
//!
//! 偏移态集合可归一化（排序、合并、primary 归属）；
//! 锚点态集合由偏移态按当前快照锚定得到，本身不再排序，保留原顺序与 primary。
//!
//! **Zero-copy 纪律**：内部存储为 `Arc<[Selection<T>]>`，`Clone` 是 O(1) 引用计数递增。

use std::sync::Arc;

use zcv_multi_buffer::{MultiBufferAnchor, MultiBufferOffset, MultiBufferSnapshot};
use zcv_text::{Affinity, ByteOffset, PositionMap};

use super::Selection;

/// 归一化后的多选区 / 多光标集合。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SelectionSet<T = MultiBufferOffset> {
    selections: Arc<[Selection<T>]>,
    primary_index: usize,
}

impl<T: Copy> SelectionSet<T> {
    /// 由已归属的选区集合直接构造；不做归一化（锚点态无法在无快照时排序）。
    pub(crate) fn from_selections(selections: Vec<Selection<T>>, primary_index: usize) -> Self {
        assert!(!selections.is_empty(), "选区集合不能为空");
        let primary_index = primary_index.min(selections.len() - 1);
        Self {
            selections: Arc::from(selections),
            primary_index,
        }
    }

    pub fn caret(offset: T) -> Self {
        Self {
            selections: Arc::from(vec![Selection::caret(offset)]),
            primary_index: 0,
        }
    }

    pub fn as_slice(&self) -> &[Selection<T>] {
        &self.selections
    }

    pub fn len(&self) -> usize {
        self.selections.len()
    }

    pub fn primary_index(&self) -> usize {
        self.primary_index
    }

    pub fn primary(&self) -> &Selection<T> {
        &self.selections[self.primary_index]
    }
}

impl<T: Copy + Ord + Default> SelectionSet<T> {
    /// 空集合会被规范化为文首单 caret。
    pub fn new(selections: Vec<Selection<T>>) -> Self {
        Self::new_with_primary(selections, 0)
    }

    pub fn new_with_primary(selections: Vec<Selection<T>>, primary_index: usize) -> Self {
        normalize_selections(selections, primary_index)
    }

    pub fn normalized(&self) -> Self {
        // 归一化是纯函数；如果当前已经归一化，复制 Arc 即可（外部观察一致）。
        Self::new_with_primary(
            self.selections.iter().copied().collect(),
            self.primary_index,
        )
    }
}

impl SelectionSet<MultiBufferOffset> {
    pub fn map_through_position_map(&self, position_map: &PositionMap) -> Self {
        // 批量映射：收集全部端点排序后单遍推进，替代逐 selection 各自线性扫描，映射成本从 O(A×E) 降为 O(A log A + E)。
        let selection_count = self.selections.len();
        let mut points: Vec<(MultiBufferOffset, usize, bool)> =
            Vec::with_capacity(selection_count * 2);
        for (index, selection) in self.selections.iter().copied().enumerate() {
            points.push((selection.start(), index, true));
            points.push((selection.end(), index, false));
        }
        points.sort_unstable_by_key(|(offset, ..)| *offset);
        let offsets: Vec<ByteOffset> = points
            .iter()
            .map(|(offset, ..)| ByteOffset::new(offset.get()))
            .collect();
        let results = position_map.map_old_positions(&offsets, Affinity::After);

        let mut starts = vec![MultiBufferOffset::ZERO; selection_count];
        let mut ends = vec![MultiBufferOffset::ZERO; selection_count];
        for ((_, index, is_start), result) in points.iter().zip(results) {
            let offset = result.value();
            if *is_start {
                starts[*index] = offset.into();
            } else {
                ends[*index] = offset.into();
            }
        }

        Self::from_selections(
            self.selections
                .iter()
                .copied()
                .enumerate()
                .map(|(index, selection)| {
                    Selection::from_parts(
                        starts[index],
                        ends[index],
                        selection.reversed(),
                        selection.goal(),
                    )
                })
                .collect(),
            self.primary_index,
        )
    }

    /// 把偏移选区集合按其所属快照锚定为源锚点集合；顺序与 primary 保持不变。
    pub(crate) fn anchored(
        self,
        snapshot: &MultiBufferSnapshot,
    ) -> SelectionSet<MultiBufferAnchor> {
        SelectionSet::from_selections(
            self.selections
                .iter()
                .copied()
                .map(|selection| selection.anchored(snapshot))
                .collect(),
            self.primary_index,
        )
    }
}

impl SelectionSet<MultiBufferAnchor> {
    /// 按当前快照把源锚点集合解析为偏移集合；顺序与 primary 保持不变。
    ///
    /// 端点版本已被 reset / 基线替换淘汰的选区无法表示，直接丢弃；
    /// 全部不可解析时退化为默认单光标（选区集合必须非空）。
    pub(crate) fn resolve(
        &self,
        snapshot: &MultiBufferSnapshot,
    ) -> SelectionSet<MultiBufferOffset> {
        let resolved: Vec<_> = self
            .selections
            .iter()
            .filter_map(|selection| selection.resolve(snapshot))
            .collect();
        if resolved.is_empty() {
            return SelectionSet::default();
        }
        SelectionSet::from_selections(resolved, self.primary_index)
    }

    /// 外部 reload / 基线替换后把全部端点显式重锚到当前快照；只由 Editor 的 reload 恢复路径使用。
    pub(crate) fn reattach(&self, snapshot: &MultiBufferSnapshot) -> Self {
        SelectionSet::from_selections(
            self.selections
                .iter()
                .copied()
                .map(|selection| selection.reattach(snapshot))
                .collect(),
            self.primary_index,
        )
    }
}

impl Default for SelectionSet<MultiBufferOffset> {
    fn default() -> Self {
        Self::caret(MultiBufferOffset::ZERO)
    }
}

fn normalize_selections<T: Copy + Ord + Default>(
    selections: Vec<Selection<T>>,
    primary_index: usize,
) -> SelectionSet<T> {
    if selections.is_empty() {
        return SelectionSet::caret(T::default());
    }

    let original_primary_index = primary_index.min(selections.len() - 1);
    let original_primary_head = selections[original_primary_index].head();

    let mut indexed: Vec<(usize, Selection<T>)> = selections.into_iter().enumerate().collect();
    indexed.sort_by_key(|(_, selection)| {
        (
            selection.start(),
            selection.end(),
            selection.head(),
            selection.tail(),
        )
    });

    let mut merged: Vec<Selection<T>> = Vec::new();

    for (_, selection) in indexed {
        let Some(current) = merged.last_mut() else {
            merged.push(selection);
            continue;
        };

        if current.end() >= selection.start() {
            let start = current.start().min(selection.start());
            let end = current.end().max(selection.end());
            *current = Selection::new(start, end);
        } else {
            merged.push(selection);
        }
    }

    let primary_index = merged
        .iter()
        .position(|selection| contains_offset(*selection, original_primary_head))
        .unwrap_or_else(|| {
            merged
                .iter()
                .position(|selection| selection.head() >= original_primary_head)
                .unwrap_or(merged.len() - 1)
        });

    SelectionSet {
        selections: Arc::from(merged),
        primary_index,
    }
}

fn contains_offset<T: Copy + Ord>(selection: Selection<T>, offset: T) -> bool {
    selection.start() <= offset && offset <= selection.end()
}

#[cfg(test)]
#[path = "test/selection_set_tests.rs"]
mod tests;
