//! BufferConfig 聚合层：把独立策略组合成创建 Buffer 时的一组默认行为。
//!
//! 本文件只做配置拼装和默认值，不把策略应用到文本，也不读取宿主环境。

use std::num::NonZeroUsize;

use super::{
    DisplayWidthPolicy, EncodingConfig, LargeFilePolicy, LineEndingConfig, PositionEncodingConfig,
    WordBoundaryPolicy,
};

/// Buffer 级别的综合配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferConfig {
    /// Tab 展示宽度与缩进输入策略。
    pub tab: TabConfig,
    /// 保存或规范化文本时使用的换行策略。
    pub line_ending: LineEndingConfig,
    /// 外部 bytes 加载为 Buffer 文本时的编码恢复策略。
    pub encoding: EncodingConfig,
    /// 与外部协议交换位置时采用的行内坐标编码。
    pub position_encoding: PositionEncodingConfig,
    /// 大文件、长行和历史保留相关的降级阈值。
    pub large_file: LargeFilePolicy,
    /// 纯文本 display column 数学使用的字符宽度策略。
    pub display_width: DisplayWidthPolicy,
    /// Word / Identifier / Symbol movement 使用的字符分类策略。
    pub word_boundary: WordBoundaryPolicy,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            tab: TabConfig::default(),
            line_ending: LineEndingConfig::Preserve,
            encoding: EncodingConfig::default(),
            position_encoding: PositionEncodingConfig::Utf8,
            large_file: LargeFilePolicy::default(),
            display_width: DisplayWidthPolicy::default(),
            word_boundary: WordBoundaryPolicy::default(),
        }
    }
}

/// Tab 与缩进策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabConfig {
    /// 制表符的视觉列宽，必须大于 0。
    pub tab_width: NonZeroUsize,
    /// 自动缩进的宽度，必须大于 0。
    pub indent_width: NonZeroUsize,
    /// 缩进时是否使用空格替代真实的 '\t'。
    pub insert_spaces: bool,
}

impl TabConfig {
    pub fn new(tab_width: NonZeroUsize, indent_width: NonZeroUsize, insert_spaces: bool) -> Self {
        Self {
            tab_width,
            indent_width,
            insert_spaces,
        }
    }

    pub fn tab_width(self) -> usize {
        self.tab_width.get()
    }

    pub fn indent_width(self) -> usize {
        self.indent_width.get()
    }
}

impl Default for TabConfig {
    fn default() -> Self {
        Self {
            tab_width: NonZeroUsize::new(4).expect("默认 tab width 必须大于 0"),
            indent_width: NonZeroUsize::new(4).expect("默认 indent width 必须大于 0"),
            insert_spaces: true,
        }
    }
}
