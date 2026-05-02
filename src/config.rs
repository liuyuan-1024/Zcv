//! 引擎配置与策略系统。

use std::num::NonZeroUsize;

/// Buffer 级别的综合配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferConfig {
    pub tab: TabConfig,
    pub line_ending: LineEndingConfig,
    pub position_encoding: PositionEncodingConfig,
    pub large_file: LargeFilePolicy,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            tab: TabConfig::default(),
            line_ending: LineEndingConfig::Preserve,
            position_encoding: PositionEncodingConfig::Utf8,
            large_file: LargeFilePolicy::default(),
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

/// 换行符策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEndingConfig {
    /// 强制使用 LF (`\n`)。
    Lf,
    /// 强制使用 CRLF (`\r\n`)。
    Crlf,
    /// 保留原文件换行风格。
    Preserve,
    /// 使用当前平台的原生换行风格。
    Native,
}

/// 坐标编码与外部通信策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionEncodingConfig {
    Utf8,
    Utf16,
    Utf32,
}

/// 大文件与降级策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeFilePolicy {
    /// 触发大文件降级策略的阈值，单位是字节。
    pub threshold_bytes: usize,
    /// 触发超长行降级策略的阈值，单位是字节。
    pub long_line_threshold_bytes: usize,
    /// 最大允许保留的 Undo 历史节点数。
    pub max_undo_history: usize,
    /// 是否启用高成本内部索引。
    pub enable_expensive_indices: bool,
    /// 是否允许向外部分析系统暴露“建议降级”的提示。
    pub allow_external_analysis_hints: bool,
}

impl Default for LargeFilePolicy {
    fn default() -> Self {
        Self {
            threshold_bytes: 5 * 1024 * 1024,
            long_line_threshold_bytes: 512 * 1024,
            max_undo_history: 1000,
            enable_expensive_indices: true,
            allow_external_analysis_hints: true,
        }
    }
}
