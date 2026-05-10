//! Edit：事务系统的最小文本变异单元。
//!
//! Edit 使用旧文本 CharOffset/TextRange 坐标，不直接检查 Buffer 边界或提交原子性。
//! 字段封装在内部：构造器 (`new` / `insert` / `delete` / `replace`) 是合法 Edit 的唯一入口，
//! 外部代码只能通过 `range()` / `replacement()` 只读访问，避免在已构造的 Edit 上越过
//! `EditList::new` 排序与不重叠校验偷偷篡改坐标或文本。

use crate::{
    errors::CoordinateError,
    types::{CharOffset, TextRange},
};

/// 描述单次文本修改。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Edit {
    range: TextRange,
    replacement: String,
}

impl Edit {
    pub fn new(range: TextRange, replacement: String) -> Self {
        Self { range, replacement }
    }

    pub fn insert(offset: CharOffset, text: String) -> Result<Self, CoordinateError> {
        Ok(Self {
            range: TextRange::new(offset, offset)?,
            replacement: text,
        })
    }

    pub fn delete(range: TextRange) -> Self {
        Self {
            range,
            replacement: String::new(),
        }
    }

    pub fn replace(range: TextRange, replacement: String) -> Self {
        Self { range, replacement }
    }

    /// 旧文本中的 CharOffset 半开区间；插入用空区间表示。
    pub fn range(&self) -> TextRange {
        self.range
    }

    /// 替换文本，按 Unicode scalar 数参与后续 changed ranges 和 PositionMap 计算。
    pub fn replacement(&self) -> &str {
        &self.replacement
    }
}
