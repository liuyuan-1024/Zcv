//! 纯文本显示列策略：定义字符宽度估算和 display column 吸附规则。
//!
//! 这里处理的是编辑引擎的列宽数学，不承诺字体 shaping、ligature 或真实像素测量。
//! 字符宽度判定委托给 `unicode-width`（与 Zed 同款），不再手写 Unicode 宽度表。

use unicode_width::UnicodeWidthChar;

/// display column 落在一个多列字符或 tab 展开区间中间时的吸附策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DisplayColumnAffinity {
    /// 吸附到前一个合法 logical column。
    Previous,
    /// 吸附到后一个合法 logical column。
    Next,
    /// 吸附到距离更近的合法 logical column；距离相等时选择前一个。
    #[default]
    Nearest,
}

/// 基础字符显示宽度策略。
///
/// 只负责纯文本层面的列宽数学，不负责真实像素测量、字体 shaping、ligature 或渲染器布局。
/// 各字符类别的宽度判定由 `unicode-width` 按 Unicode 标准给出，不再保留可配置宽度字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayWidthPolicy {
    /// display column -> logical column 的默认吸附策略。
    pub affinity: DisplayColumnAffinity,
}

impl DisplayWidthPolicy {
    pub const fn new(affinity: DisplayColumnAffinity) -> Self {
        Self { affinity }
    }

    pub fn char_width(self, ch: char) -> usize {
        // 控制字符（含换行、tab）与组合音标、ZWJ、变体选择符等零宽字符宽度为 0；
        // emoji 与东亚宽字符宽度为 2；其余（含 East Asian Ambiguous）为 1。
        UnicodeWidthChar::width(ch).unwrap_or(0)
    }
}

impl Default for DisplayWidthPolicy {
    fn default() -> Self {
        Self {
            affinity: DisplayColumnAffinity::Nearest,
        }
    }
}

/// 零宽字符（组合音标、ZWJ、变体选择符等）判定。
///
/// 这类字符不占显示列宽，并应与其前后字符保持为一个词。
/// 控制字符宽度未定义（返回 `None`），不属于零宽字符，由调用方按控制字符处理。
pub(crate) fn is_zero_width(ch: char) -> bool {
    UnicodeWidthChar::width(ch) == Some(0)
}
