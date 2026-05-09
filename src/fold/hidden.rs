//! HiddenRange：FoldSet 投影出的「被折叠隐藏」逻辑行区间。
//!
//! HiddenRange 只表达「哪些逻辑行不可见」事实；fold 占位符样式与投影坐标属于 M13B 起。

use crate::types::{Line, LineRange};

/// 一段连续被隐藏的逻辑行区间。
///
/// 半开区间 `[start, end)`，其中 `start` 是第一条隐藏的逻辑行，`end` 是该隐藏段后
/// 第一条仍然可见的逻辑行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HiddenRange {
    lines: LineRange,
}

impl HiddenRange {
    pub fn new(lines: LineRange) -> Self {
        Self { lines }
    }

    pub fn lines(self) -> LineRange {
        self.lines
    }

    pub fn first_hidden_line(self) -> Line {
        self.lines.start()
    }

    pub fn end_line_exclusive(self) -> Line {
        self.lines.end()
    }

    pub fn len(self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(self) -> bool {
        self.lines.is_empty()
    }

    pub fn contains_line(self, line: Line) -> bool {
        self.lines.start() <= line && line < self.lines.end()
    }
}
