//! SelectionSet：多光标/多选区的归一化集合。
//!
//! 本文件维护排序、合并和 primary selection 归属，是 Buffer 唯一的选区模型。
//!
//! **Zero-copy 纪律**：内部存储为 `Arc<[Selection]>`，`Clone` 是 O(1) 引用计数递增。

use std::sync::Arc;

use super::Selection;
use crate::{ByteOffset, PositionMap, TextRange, position_map::Affinity};

/// 选区归一化时对相邻区间的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SelectionMergePolicy {
    /// 只合并重叠区间、重复 caret、以及落在选区边界上的 caret。
    #[default]
    MergeOverlapping,
    /// 额外合并首尾相接的非空区间。
    MergeOverlappingOrAdjacent,
}

/// 归一化后的多选区 / 多光标集合。
///
/// 内部 `Arc<[Selection]>`：`Clone` 是 O(1)，宿主在编辑事务前后传递时零深拷贝。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SelectionSet {
    selections: Arc<[Selection]>,
    primary_index: usize,
}

impl SelectionSet {
    /// 空集合会被规范化为文首单 caret。
    pub fn new(selections: Vec<Selection>) -> Self {
        Self::new_with_primary(selections, 0)
    }

    pub fn new_with_primary(selections: Vec<Selection>, primary_index: usize) -> Self {
        Self::new_with_policy(
            selections,
            primary_index,
            SelectionMergePolicy::MergeOverlapping,
        )
    }

    pub fn new_with_policy(
        selections: Vec<Selection>,
        primary_index: usize,
        policy: SelectionMergePolicy,
    ) -> Self {
        normalize_selections(selections, primary_index, policy)
    }

    pub fn caret(offset: ByteOffset) -> Self {
        Self {
            selections: Arc::from(vec![Selection::caret(offset)]),
            primary_index: 0,
        }
    }

    pub fn from_ranges(ranges: Vec<TextRange>) -> Self {
        Self::new(
            ranges
                .into_iter()
                .map(|range| Selection::new(range.start(), range.end()))
                .collect(),
        )
    }

    pub fn as_slice(&self) -> &[Selection] {
        &self.selections
    }

    pub fn len(&self) -> usize {
        self.selections.len()
    }

    pub fn is_empty(&self) -> bool {
        self.selections.is_empty()
    }

    pub fn primary_index(&self) -> usize {
        self.primary_index
    }

    pub fn primary(&self) -> &Selection {
        &self.selections[self.primary_index]
    }

    pub fn ranges(&self) -> Vec<TextRange> {
        self.selections
            .iter()
            .copied()
            .map(Selection::range)
            .collect()
    }

    pub fn normalized(&self) -> Self {
        // 归一化是纯函数；如果当前已经归一化，复制 Arc 即可（外部观察一致）。
        // 这里仍走 new_with_primary 以保证语义不变；其内部会做排序 / 合并。
        Self::new_with_primary(
            self.selections.iter().copied().collect(),
            self.primary_index,
        )
    }

    pub fn map_through_position_map(&self, position_map: &PositionMap) -> Self {
        // 批量映射：收集全部 anchor/head 点排序后单遍推进，替代逐 selection 各自线性扫描，映射成本从 O(A×E) 降为 O(A log A + E)。
        let selection_count = self.selections.len();
        let mut points: Vec<(ByteOffset, usize, bool)> = Vec::with_capacity(selection_count * 2);
        for (index, selection) in self.selections.iter().copied().enumerate() {
            points.push((selection.anchor(), index, true));
            points.push((selection.head(), index, false));
        }
        points.sort_unstable_by_key(|(offset, ..)| *offset);
        let offsets: Vec<ByteOffset> = points.iter().map(|(offset, ..)| *offset).collect();
        let results = position_map.map_old_positions(&offsets, Affinity::After);

        let mut anchors = vec![ByteOffset::ZERO; selection_count];
        let mut heads = vec![ByteOffset::ZERO; selection_count];
        for ((_, index, is_anchor), result) in points.iter().zip(results) {
            let offset = result.value();
            if *is_anchor {
                anchors[*index] = offset;
            } else {
                heads[*index] = offset;
            }
        }

        Self::new_with_primary(
            self.selections
                .iter()
                .copied()
                .enumerate()
                .map(|(index, selection)| {
                    Selection::new(anchors[index], heads[index]).with_goal(selection.goal())
                })
                .collect(),
            self.primary_index,
        )
    }
}

impl Default for SelectionSet {
    fn default() -> Self {
        Self::caret(ByteOffset::ZERO)
    }
}

fn normalize_selections(
    selections: Vec<Selection>,
    primary_index: usize,
    policy: SelectionMergePolicy,
) -> SelectionSet {
    if selections.is_empty() {
        return SelectionSet::caret(ByteOffset::ZERO);
    }

    let original_primary_index = primary_index.min(selections.len() - 1);
    let original_primary_head = selections[original_primary_index].head();

    let mut indexed: Vec<(usize, Selection)> = selections.into_iter().enumerate().collect();
    indexed.sort_by_key(|(_, selection)| {
        (
            selection.start(),
            selection.end(),
            selection.head(),
            selection.anchor(),
        )
    });

    let mut merged: Vec<Selection> = Vec::new();

    for (_, selection) in indexed {
        let Some(current) = merged.last_mut() else {
            merged.push(selection);
            continue;
        };

        if should_merge(*current, selection, policy) {
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

fn should_merge(current: Selection, next: Selection, policy: SelectionMergePolicy) -> bool {
    if current.end() > next.start() {
        return true;
    }

    if current.end() == next.start() {
        return match policy {
            SelectionMergePolicy::MergeOverlappingOrAdjacent => true,
            SelectionMergePolicy::MergeOverlapping => current.is_caret() || next.is_caret(),
        };
    }

    false
}

fn contains_offset(selection: Selection, offset: ByteOffset) -> bool {
    selection.start() <= offset && offset <= selection.end()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(value: usize) -> ByteOffset {
        ByteOffset::new(value)
    }

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(b(start), b(end)).unwrap()
    }

    fn selection(anchor: usize, head: usize) -> Selection {
        Selection::new(b(anchor), b(head))
    }

    fn caret(offset: usize) -> Selection {
        Selection::caret(b(offset))
    }

    #[test]
    fn selection_set_normalization_should_sort_merge_duplicates_and_preserve_primary() {
        let set = SelectionSet::new_with_primary(
            vec![caret(8), selection(4, 2), caret(1), selection(3, 6)],
            1,
        );

        assert_eq!(set.ranges(), vec![range(1, 1), range(2, 6), range(8, 8)]);
        assert_eq!(set.primary_index(), 1);
        assert_eq!(set.primary().range(), range(2, 6));

        let adjacent = SelectionSet::new_with_policy(
            vec![selection(1, 3), selection(3, 5)],
            0,
            SelectionMergePolicy::MergeOverlappingOrAdjacent,
        );
        assert_eq!(adjacent.ranges(), vec![range(1, 5)]);
    }
}
