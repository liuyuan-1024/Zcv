//! 显示列坐标与字符宽度：DisplayColumn 类型 + 显示列宽度的字符级估算。
//!
//! 这里处理的是显示列的列宽数学，不承诺字体 shaping、ligature 或真实像素测量。

use unicode_width::UnicodeWidthChar;

/// 视觉列号，0-indexed。
///
/// 表示考虑 Tab 展开、CJK 宽度、emoji 宽度等策略后的显示列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub(crate) struct DisplayColumn(usize);

impl DisplayColumn {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

/// 字符显示宽度：控制字符（含换行、tab）与组合音标、ZWJ、变体选择符等零宽字符宽度为 0；
/// emoji 与东亚宽字符宽度为 2；其余（含 East Asian Ambiguous）为 1。
pub(crate) fn char_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}
