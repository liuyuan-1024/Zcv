//! ChangeSet：保存一次已验证事务的编辑事实，并派生 changed ranges / PositionMap。
//!
//! 它只能从 EditList 构造，不负责排序、重叠检测或 Buffer 版本推进。

use crate::{
    position_map::PositionMap,
    types::{CharOffset, TextRange},
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
        let mut diff = 0isize;

        for edit in &self.edits {
            let old_start = edit.range.start().get() as isize;
            let old_end = edit.range.end().get() as isize;
            let replacement_len = edit.replacement.chars().count() as isize;

            let new_start = (old_start + diff).max(0) as usize;
            let new_end = new_start + replacement_len as usize;

            ranges.push(
                TextRange::new(CharOffset::new(new_start), CharOffset::new(new_end))
                    .expect("ChangeSet 生成的范围必须满足起始位置 <= 结束位置"),
            );

            diff += replacement_len - (old_end - old_start);
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
