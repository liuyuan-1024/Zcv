//! ChangeSet：保存一次已验证事务的编辑事实，并派生 changed ranges / PositionMap。
//!
//! 它只能从 EditList 构造，不负责排序、重叠检测或 Buffer 版本推进。

use crate::{
    position_map::{OffsetShift, PositionMap},
    types::TextRange,
};

use super::{Edit, EditList};

/// 事务变更集合。
///
/// `ChangeSet` 记录一次事务提交的已验证编辑，用于计算 changed ranges，并可产出
/// `PositionMap`。具体位置映射 API 统一由 `PositionMap` 承担。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    edits: Vec<Edit>,
}

impl ChangeSet {
    /// 只能从已经排序、已经验证过的 EditList 构造。
    pub(crate) fn from_edit_list(edits: &EditList) -> Self {
        Self {
            edits: edits.as_slice().to_vec(),
        }
    }

    pub(crate) fn edits(&self) -> &[Edit] {
        &self.edits
    }

    pub fn position_map(&self) -> PositionMap {
        PositionMap::from_edits(self.edits.clone())
    }

    /// 获取本次事务应用后，在新文本中发生改变的范围列表。
    pub fn changed_ranges(&self) -> Vec<TextRange> {
        if self.edits.is_empty() {
            return Vec::new();
        }

        let mut ranges = Vec::new();
        let mut shift = OffsetShift::ZERO;

        for edit in &self.edits {
            let range = edit.range();
            let replacement_len = edit.replacement().len();

            let new_start = shift.apply_old_to_new(range.start()).expect(
                "internal invariant: changed range start maps without byte offset overflow",
            );
            let new_end = new_start
                .checked_add(replacement_len)
                .expect("internal invariant: changed range end maps without byte offset overflow");

            ranges.push(
                TextRange::new(new_start, new_end)
                    .expect("ChangeSet 生成的范围必须满足起始位置 <= 结束位置"),
            );

            shift = shift
                .after_edit(range.len(), replacement_len)
                .expect("internal invariant: accumulated changed range shift does not overflow");
        }

        Self::merge_ranges(ranges)
    }

    fn merge_ranges(ranges: Vec<TextRange>) -> Vec<TextRange> {
        let mut merged = Vec::with_capacity(ranges.len());
        let mut iter = ranges.into_iter();

        let Some(mut current) = iter.next() else {
            return merged;
        };

        for next in iter {
            if current.end() >= next.start() {
                current = TextRange::new(current.start(), current.end().max(next.end()))
                    .expect("合并范围必须满足起始位置 <= 结束位置");
            } else {
                merged.push(current);
                current = next;
            }
        }

        merged.push(current);
        merged
    }
}
