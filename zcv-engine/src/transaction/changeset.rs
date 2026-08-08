//! ChangeSet：保存一次已验证事务的编辑事实，并派生 changed ranges / PositionMap。
//!
//! 它只能从 EditList 构造，不负责排序、重叠检测或 Buffer 版本推进。

use crate::{
    EngineError, EngineResult,
    position_map::{OffsetShift, PositionMap},
    types::{ByteOffset, TextRange},
};

use super::{Edit, EditList};

/// 事务变更集合。
///
/// `ChangeSet` 记录一次事务提交的已验证编辑，用于计算 changed ranges，并可产出`PositionMap`。
/// 具体位置映射 API 统一由 `PositionMap` 承担。
///
/// 内部直接持有 `EditList`（`Arc<[Edit]>`），`Clone` 与提交传递只递增引用计数，同一批编辑不再在 ChangeSet / DeltaEvent / PositionMap 之间逐元素拷贝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    edits: EditList,
}

impl ChangeSet {
    /// 只能从已经排序、已经验证过的 EditList 构造；O(1) 共享底层编辑切片。
    pub(crate) fn from_edit_list(edits: &EditList) -> Self {
        Self {
            edits: edits.clone(),
        }
    }

    /// 已排序、不重叠的事务编辑切片，使用**旧文本** ByteOffset 坐标。
    ///
    /// 提供给 syntax / tracking 等 producer 自行翻译为外部协议所需的形态（例如 tree-sitter `InputEdit` 需要旧端 + 新端 Point）。
    /// 每条 `Edit` 暴露的 `range()` / `replacement()` 已是只读访问，外部无法越过 `EditList::new` 的排序与不重叠校验。
    pub fn edits(&self) -> &[Edit] {
        self.edits.as_slice()
    }

    pub fn position_map(&self) -> PositionMap {
        PositionMap::from_edits(self.edits.as_slice())
    }

    /// 获取本次事务应用后，在新文本中发生改变的范围列表（相邻范围已原地合并）。
    pub fn changed_ranges(&self) -> EngineResult<Vec<TextRange>> {
        let mut ranges: Vec<TextRange> = Vec::new();
        let mut shift = OffsetShift::ZERO;

        for edit in self.edits.as_slice() {
            let range = edit.range();
            let replacement_len = edit.replacement().len();

            let new_start =
                shift
                    .apply_old_to_new(range.start())
                    .ok_or_else(|| EngineError::EngineBug {
                        location: "ChangeSet::changed_ranges",
                        detail: "changed range 起点在字节偏移映射时溢出".to_string(),
                    })?;
            let new_end =
                new_start
                    .checked_add(replacement_len)
                    .ok_or_else(|| EngineError::EngineBug {
                        location: "ChangeSet::changed_ranges",
                        detail: "changed range 终点在字节偏移映射时溢出".to_string(),
                    })?;
            let next = text_range(new_start, new_end, "ChangeSet::changed_ranges")?;

            // 相邻范围原地合并：与上一段相接（end >= start）时直接扩尾，否则入列。
            if let Some(current) = ranges.last_mut()
                && current.end() >= next.start()
            {
                *current = text_range(
                    current.start(),
                    current.end().max(next.end()),
                    "ChangeSet::changed_ranges",
                )?;
            } else {
                ranges.push(next);
            }

            shift = shift
                .after_edit(range.len(), replacement_len)
                .ok_or_else(|| EngineError::EngineBug {
                    location: "ChangeSet::changed_ranges",
                    detail: "累计 changed range 位移溢出".to_string(),
                })?;
        }

        Ok(ranges)
    }
}

fn text_range(
    start: ByteOffset,
    end: ByteOffset,
    location: &'static str,
) -> EngineResult<TextRange> {
    TextRange::new(start, end).map_err(|_| EngineError::EngineBug {
        location,
        detail: format!("生成了非法区间：start {start}，end {end}"),
    })
}
