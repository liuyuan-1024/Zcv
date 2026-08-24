//! Edit：事务系统的最小文本变异单元。
//!
//! Edit 使用旧文本 ByteOffset/TextRange 坐标，不直接检查 Buffer 边界或提交原子性。
//! 字段封装在内部：构造器 (`new` / `insert` / `delete` / `replace`) 是合法 Edit 的唯一入口，
//! 外部代码只能通过 `range()` / `replacement()` 只读访问，避免在已构造的 Edit 上越过
//! `EditList::new` 排序与不重叠校验偷偷篡改坐标或文本。

use std::sync::{Arc, OnceLock};

use crate::{
    errors::CoordinateError,
    types::{ByteOffset, TextRange},
};

/// 描述单次文本修改。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Edit {
    range: TextRange,
    replacement: Arc<str>,
}

impl Edit {
    pub fn new<R>(range: TextRange, replacement: R) -> Self
    where
        R: Into<Arc<str>>,
    {
        Self {
            range,
            replacement: replacement.into(),
        }
    }

    pub fn insert<R>(offset: ByteOffset, text: R) -> Result<Self, CoordinateError>
    where
        R: Into<Arc<str>>,
    {
        Ok(Self {
            range: TextRange::new(offset, offset)?,
            replacement: text.into(),
        })
    }

    pub fn delete(range: TextRange) -> Self {
        Self {
            range,
            replacement: empty_replacement(),
        }
    }

    pub fn replace<R>(range: TextRange, replacement: R) -> Self
    where
        R: Into<Arc<str>>,
    {
        Self {
            range,
            replacement: replacement.into(),
        }
    }

    /// 旧文本中的 ByteOffset 半开区间；插入用空区间表示。
    pub fn range(&self) -> TextRange {
        self.range
    }

    /// 替换文本，按 UTF-8 字节长度参与后续 changed ranges 和 PositionMap 计算。
    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    pub(super) fn replacement_arc(&self) -> &Arc<str> {
        &self.replacement
    }

    pub(super) fn share_replacement_with(&mut self, replacement: Arc<str>) {
        self.replacement = replacement;
    }
}

pub(super) fn empty_replacement() -> Arc<str> {
    static EMPTY: OnceLock<Arc<str>> = OnceLock::new();
    Arc::clone(EMPTY.get_or_init(|| Arc::from("")))
}
