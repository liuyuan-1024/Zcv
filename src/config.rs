//! 引擎配置与策略系统。

use std::num::NonZeroUsize;

/// Buffer 级别的综合配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferConfig {
    pub tab: TabConfig,
    pub line_ending: LineEndingConfig,
    pub position_encoding: PositionEncodingConfig,
    pub large_file: LargeFilePolicy,
    pub display_width: DisplayWidthPolicy,
    pub word_boundary: WordBoundaryPolicy,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            tab: TabConfig::default(),
            line_ending: LineEndingConfig::Preserve,
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
/// M5B 只负责纯文本层面的列宽数学，不负责真实像素测量、字体 shaping、ligature 或渲染器布局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayWidthPolicy {
    /// display column -> logical column 的默认吸附策略。
    pub affinity: DisplayColumnAffinity,
    /// CJK 宽字符的显示宽度。
    pub cjk_width: usize,
    /// emoji / pictographic 字符的显示宽度。
    pub emoji_width: usize,
    /// East Asian Ambiguous 字符的显示宽度。
    pub ambiguous_width: usize,
    /// 非换行控制字符的显示宽度。
    pub control_width: usize,
    /// 组合音标等 mark 字符的显示宽度。
    pub combining_mark_width: usize,
}

impl DisplayWidthPolicy {
    pub const fn new(
        affinity: DisplayColumnAffinity,
        cjk_width: usize,
        emoji_width: usize,
        ambiguous_width: usize,
        control_width: usize,
        combining_mark_width: usize,
    ) -> Self {
        Self {
            affinity,
            cjk_width,
            emoji_width,
            ambiguous_width,
            control_width,
            combining_mark_width,
        }
    }

    pub fn char_width(self, ch: char) -> usize {
        if ch == '\n' || ch == '\r' {
            0
        } else if is_combining_mark(ch) {
            self.combining_mark_width
        } else if ch.is_control() {
            self.control_width
        } else if is_emoji_like(ch) {
            self.emoji_width
        } else if is_cjk_wide(ch) {
            self.cjk_width
        } else if is_east_asian_ambiguous(ch) {
            self.ambiguous_width
        } else {
            1
        }
    }
}

impl Default for DisplayWidthPolicy {
    fn default() -> Self {
        Self {
            affinity: DisplayColumnAffinity::Nearest,
            cjk_width: 2,
            emoji_width: 2,
            ambiguous_width: 1,
            control_width: 0,
            combining_mark_width: 0,
        }
    }
}

/// M6B 词边界策略。
///
/// 引擎层只定义纯文本移动语义，不绑定具体 UI 快捷键。不同宿主可以把
/// Option/Alt/Ctrl + Left/Right 映射到 Word / Identifier / Subword / Symbol。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordBoundaryPolicy {
    /// `_` 是否视为 identifier 的一部分。
    ///
    /// 默认开启，适配 `snake_case`、Rust / C / JS 常见标识符。
    pub underscore_is_identifier: bool,
    /// `$` 是否视为 identifier 的一部分。
    ///
    /// 默认开启，适配 JS / shell / 部分模板语言常见标识符。
    pub dollar_is_identifier: bool,
    /// ASCII apostrophe 是否允许出现在自然语言 word 内。
    ///
    /// 当前 M6B 的 Unicode word movement 主要依赖 `unicode-segmentation`，该字段
    /// 保留给后续更细的自然语言策略；identifier / subword / symbol 不使用它。
    pub apostrophe_is_word: bool,
}

impl WordBoundaryPolicy {
    pub const fn new(
        underscore_is_identifier: bool,
        dollar_is_identifier: bool,
        apostrophe_is_word: bool,
    ) -> Self {
        Self {
            underscore_is_identifier,
            dollar_is_identifier,
            apostrophe_is_word,
        }
    }

    pub(crate) fn is_identifier_continue(self, ch: char) -> bool {
        ch.is_alphanumeric()
            || is_combining_mark(ch)
            || (self.underscore_is_identifier && ch == '_')
            || (self.dollar_is_identifier && ch == '$')
    }

    pub(crate) fn is_symbol_char(self, ch: char) -> bool {
        !ch.is_whitespace() && !self.is_identifier_continue(ch)
    }
}

impl Default for WordBoundaryPolicy {
    fn default() -> Self {
        Self {
            underscore_is_identifier: true,
            dollar_is_identifier: true,
            apostrophe_is_word: false,
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

fn is_combining_mark(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F
    )
}

fn is_cjk_wide(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x115F
            | 0x2329..=0x232A
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
    )
}

fn is_emoji_like(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1F000..=0x1FAFF | 0x2600..=0x27BF
    )
}

fn is_east_asian_ambiguous(ch: char) -> bool {
    matches!(
        ch as u32,
        0x00A1
            | 0x00A4
            | 0x00A7..=0x00A8
            | 0x00AA
            | 0x00AD..=0x00AE
            | 0x00B0..=0x00B4
            | 0x00B6..=0x00BA
            | 0x00BC..=0x00BF
            | 0x00C6
            | 0x00D0
            | 0x00D7..=0x00D8
            | 0x00DE..=0x00E1
            | 0x00E6
            | 0x00E8..=0x00EA
            | 0x00EC..=0x00ED
            | 0x00F0
            | 0x00F2..=0x00F3
            | 0x00F7..=0x00FA
            | 0x00FC
            | 0x00FE
            | 0x0101
            | 0x0111
            | 0x0113
            | 0x011B
            | 0x0126..=0x0127
            | 0x012B
            | 0x0131..=0x0133
            | 0x0138
            | 0x013F..=0x0142
            | 0x0144
            | 0x0148..=0x014B
            | 0x014D
            | 0x0152..=0x0153
            | 0x0166..=0x0167
            | 0x016B
            | 0x01CE
            | 0x01D0
            | 0x01D2
            | 0x01D4
            | 0x01D6
            | 0x01D8
            | 0x01DA
            | 0x01DC
            | 0x0251
            | 0x0261
            | 0x02C4
            | 0x02C7
            | 0x02C9..=0x02CB
            | 0x02CD
            | 0x02D0
            | 0x02D8..=0x02DB
            | 0x02DD
            | 0x02DF
            | 0x0391..=0x03A1
            | 0x03A3..=0x03A9
            | 0x03B1..=0x03C1
            | 0x03C3..=0x03C9
            | 0x0401
            | 0x0410..=0x044F
            | 0x0451
            | 0x2010
            | 0x2013..=0x2016
            | 0x2018..=0x2019
            | 0x201C..=0x201D
            | 0x2020..=0x2022
            | 0x2024..=0x2027
            | 0x2030
            | 0x2032..=0x2033
            | 0x2035
            | 0x203B
            | 0x203E
            | 0x2074
            | 0x207F
            | 0x2081..=0x2084
            | 0x20AC
            | 0x2103
            | 0x2105
            | 0x2109
            | 0x2113
            | 0x2116
            | 0x2121..=0x2122
            | 0x2126
            | 0x212B
            | 0x2153..=0x2154
            | 0x215B..=0x215E
            | 0x2160..=0x216B
            | 0x2170..=0x2179
            | 0x2189
            | 0x2190..=0x2199
            | 0x21B8..=0x21B9
            | 0x21D2
            | 0x21D4
            | 0x21E7
            | 0x2200
            | 0x2202..=0x2203
            | 0x2207..=0x2208
            | 0x220B
            | 0x220F
            | 0x2211
            | 0x2215
            | 0x221A
            | 0x221D..=0x2220
            | 0x2223
            | 0x2225
            | 0x2227..=0x222C
            | 0x222E
            | 0x2234..=0x2237
            | 0x223C..=0x223D
            | 0x2248
            | 0x224C
            | 0x2252
            | 0x2260..=0x2261
            | 0x2264..=0x2267
            | 0x226A..=0x226B
            | 0x226E..=0x226F
            | 0x2282..=0x2283
            | 0x2286..=0x2287
            | 0x2295
            | 0x2299
            | 0x22A5
            | 0x22BF
            | 0x2312
            | 0x2460..=0x24E9
            | 0x24EB..=0x254B
            | 0x2550..=0x2573
            | 0x2580..=0x258F
            | 0x2592..=0x2595
            | 0x25A0..=0x25A1
            | 0x25A3..=0x25A9
            | 0x25B2..=0x25B3
            | 0x25B6..=0x25B7
            | 0x25BC..=0x25BD
            | 0x25C0..=0x25C1
            | 0x25C6..=0x25C8
            | 0x25CB
            | 0x25CE..=0x25D1
            | 0x25E2..=0x25E5
            | 0x25EF
            | 0x2605..=0x2606
            | 0x2609
            | 0x260E..=0x260F
            | 0x261C
            | 0x261E
            | 0x2640
            | 0x2642
            | 0x2660..=0x2661
            | 0x2663..=0x2665
            | 0x2667..=0x266A
            | 0x266C..=0x266D
            | 0x266F
            | 0x269E..=0x269F
            | 0x26BF
            | 0x26C6..=0x26CD
            | 0x26CF..=0x26D3
            | 0x26D5
            | 0x26E3
            | 0x26E8..=0x26E9
            | 0x26EB..=0x26F1
            | 0x26F4
            | 0x26F6..=0x26F9
            | 0x26FB..=0x26FC
            | 0x26FE..=0x26FF
            | 0x273D
            | 0x2776..=0x277F
            | 0x2B56..=0x2B59
            | 0x3248..=0x324F
            | 0xE000..=0xF8FF
            | 0xFE00..=0xFE0F
            | 0xFFFD
            | 0x1F100..=0x1F10A
            | 0x1F110..=0x1F12D
            | 0x1F130..=0x1F169
            | 0x1F170..=0x1F18D
            | 0x1F18F..=0x1F190
            | 0x1F19B..=0x1F1AC
            | 0x1F200..=0x1F202
            | 0x1F210..=0x1F23B
            | 0x1F240..=0x1F248
            | 0x1F250..=0x1F251
            | 0x1F260..=0x1F265
    )
}
