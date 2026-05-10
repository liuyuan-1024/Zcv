//! Metadata 行窗口：按逻辑行范围查询 metadata 的窄类型。
//!
//! 它刻意不叫 Viewport，也不表达 viewport 的滚动、投影、折叠或像素可见区。

use crate::{
    errors::CoordinateError,
    types::{Line, LineRange},
};

/// Metadata 可见行查询窗口。
///
/// 只表达一段逻辑行范围，不涉及 Viewport、UI 渲染、像素滚动或折叠投影坐标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MetadataLineWindow {
    lines: LineRange,
}

impl MetadataLineWindow {
    pub fn new(lines: LineRange) -> Self {
        Self { lines }
    }

    pub fn from_lines(start: Line, end: Line) -> Result<Self, CoordinateError> {
        Ok(Self::new(LineRange::new(start, end)?))
    }

    pub fn lines(self) -> LineRange {
        self.lines
    }
}
