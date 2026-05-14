//! SelectionSet：多光标/多选区的归一化集合。
//!
//! 本文件维护排序、合并和 primary selection 归属，是 Buffer 唯一的选区模型。
//!
//! **Zero-copy 纪律**：内部存储为 `Arc<[Selection]>`，`Clone` 是 O(1) 引用计数递增。

use std::sync::Arc;

use crate::{ByteOffset, PositionMap, TextRange};

use super::Selection;

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
/// 内部 `Arc<[Selection]>`：`Clone` 是 O(1)，事务热路径（`before_selection.clone()` /
/// `selection.clone()` 反复传递）零深拷贝。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SelectionSet {
    selections: Arc<[Selection]>,
    primary_index: usize,
}

impl SelectionSet {
    /// 创建选区集合。空集合会被规范化为文首单 caret。
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
        Self::new_with_primary(
            self.selections
                .iter()
                .copied()
                .map(|selection| selection.map_through_position_map(position_map))
                .collect(),
            self.primary_index,
        )
    }

    /// 物化为 `Vec<Selection>`。需要拥有 owned 数据时使用；命名让分配语义显眼。
    pub fn into_vec(self) -> Vec<Selection> {
        self.selections.iter().copied().collect()
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
