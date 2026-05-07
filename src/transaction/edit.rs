//! Edit：事务系统的最小文本变异单元。
//!
//! Edit 使用旧文本 CharOffset/TextRange 坐标，不直接检查 Buffer 边界或提交原子性。

use crate::{
    errors::CoordinateError,
    types::{CharOffset, TextRange},
};

/// 描述单次文本修改。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Edit {
    /// 旧文本中的 CharOffset 半开区间；插入用空区间表示。
    pub range: TextRange,
    /// 替换文本，按 Unicode scalar 数参与后续 changed ranges 和 PositionMap 计算。
    pub replacement: String,
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
}
