//! 按行索引的文本摘要：为 `Snapshot::text_summary_for_range` 提供 O(log n) 的宽度维度来源。
//!
//! 树中每一项代表一行（含其行终止符）。文本末尾因换行产生的空行不单独成项，因此有效项都至少包含一个字符或换行符，行边界在树中严格递增。
//! 每项摘要与整棵树摘要都使用同一套拼接语义（`TextSummary`），区间聚合因此只重测首尾不完整行，不扫描中间行。

use std::ops::Range;

use ropey::Rope;
use sum_tree::{Bias, ContextLessSummary, Dimension, Item, SumTree};

use crate::types::TextRange;

/// 一段文本的多维长度摘要，对齐 Zed `text::TextSummary` 的 `longest_row` 维度。
///
/// 字节、Unicode scalar、UTF-16 code unit 与换行数来自同一份文本；
/// `first_line_chars` / `last_line_chars` 只描述行宽，`longest_row` 是相对本摘要起点的行号，并列时取更早的行。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextSummary {
    /// UTF-8 字节数。
    pub len: usize,
    /// Unicode scalar 数。
    pub chars: usize,
    /// UTF-16 code unit 数。
    pub len_utf16: usize,
    /// 换行符数量。
    pub lines: usize,
    /// 第一行的 Unicode scalar 数。
    pub first_line_chars: usize,
    /// 最后一个换行符之后的 Unicode scalar 数；文本以换行结尾时为 0。
    pub last_line_chars: usize,
    /// 最长行的相对行号。
    pub longest_row: usize,
    /// 最长行的 Unicode scalar 数。
    pub longest_row_chars: usize,
}

impl TextSummary {
    /// 测量一段文本；`len` 必须与 `chars` 描述同一段文本的 UTF-8 字节数。
    fn from_chars(chars: impl Iterator<Item = char>, len: usize) -> Self {
        let mut summary = Self {
            len,
            ..Self::default()
        };
        for character in chars {
            summary.chars += 1;
            summary.len_utf16 += character.len_utf16();
            if character == '\n' {
                summary.lines += 1;
                summary.last_line_chars = 0;
            } else {
                summary.last_line_chars += 1;
            }
            if summary.lines == 0 {
                summary.first_line_chars = summary.last_line_chars;
            }
            if summary.last_line_chars > summary.longest_row_chars {
                summary.longest_row = summary.lines;
                summary.longest_row_chars = summary.last_line_chars;
            }
        }
        summary
    }

    /// 把 `other` 并接到本摘要之后。
    ///
    /// 拼接点上的两行会合并成候选最长行；
    /// `other` 不含换行时它的全部字符都落在本摘要的最后一行，否则本摘要的最后一行被 `other` 的最后一行取代。
    fn join(&mut self, other: &Self) {
        let joined_chars = self.last_line_chars + other.first_line_chars;
        if joined_chars > self.longest_row_chars {
            self.longest_row = self.lines;
            self.longest_row_chars = joined_chars;
        }
        if other.longest_row_chars > self.longest_row_chars {
            self.longest_row = self.lines + other.longest_row;
            self.longest_row_chars = other.longest_row_chars;
        }

        if self.lines == 0 {
            self.first_line_chars += other.first_line_chars;
        }
        if other.lines == 0 {
            self.last_line_chars += other.last_line_chars;
        } else {
            self.last_line_chars = other.last_line_chars;
        }

        self.len += other.len;
        self.chars += other.chars;
        self.len_utf16 += other.len_utf16;
        self.lines += other.lines;
    }
}

impl std::ops::AddAssign<&TextSummary> for TextSummary {
    fn add_assign(&mut self, other: &TextSummary) {
        self.join(other);
    }
}

impl std::ops::AddAssign<TextSummary> for TextSummary {
    fn add_assign(&mut self, other: TextSummary) {
        self.join(&other);
    }
}

impl std::ops::Add<TextSummary> for TextSummary {
    type Output = Self;

    fn add(mut self, other: TextSummary) -> Self {
        self.join(&other);
        self
    }
}

impl ContextLessSummary for TextSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.join(summary);
    }
}

/// 一行的测量值：行内容加其行终止符。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LineSummary {
    len: usize,
    chars: usize,
    len_utf16: usize,
    terminated: bool,
    /// 行终止符之前的 Unicode scalar 数。
    line_chars: usize,
}

impl LineSummary {
    fn push_char(&mut self, character: char) {
        self.len += character.len_utf8();
        self.chars += 1;
        self.len_utf16 += character.len_utf16();
        if character == '\n' {
            self.terminated = true;
        } else {
            self.line_chars += 1;
        }
    }

    fn text_summary(self) -> TextSummary {
        TextSummary {
            len: self.len,
            chars: self.chars,
            len_utf16: self.len_utf16,
            lines: usize::from(self.terminated),
            first_line_chars: self.line_chars,
            last_line_chars: if self.terminated { 0 } else { self.line_chars },
            longest_row: 0,
            longest_row_chars: self.line_chars,
        }
    }
}

impl Item for LineSummary {
    type Summary = TextSummary;

    fn summary(&self, (): ()) -> TextSummary {
        self.text_summary()
    }
}

/// 按字节累积的 seek 维度；每项的长度来自其摘要的 `len`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct LineBytes(usize);

impl<'a> Dimension<'a, TextSummary> for LineBytes {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a TextSummary, (): ()) {
        self.0 += summary.len;
    }
}

/// 文本的行摘要树。每项一行，项摘要的 `len` 是行内字节数。
#[derive(Clone, Debug, Default)]
pub(crate) struct LineIndex {
    lines: SumTree<LineSummary>,
}

impl LineIndex {
    /// 从完整文本全量构建；只在构造、整体替换等显式边界调用。
    pub(crate) fn from_text(text: &str) -> Self {
        Self {
            lines: SumTree::from_iter(measure_lines(text.chars()), ()),
        }
    }

    /// 聚合 `range` 覆盖文本的摘要。
    ///
    /// `range` 端点必须落在字符边界且不超过文本长度；首尾只重测包含边界的不完整行，
    /// 中间完整行直接取子树摘要。
    pub(crate) fn summary_for_range(&self, rope: &Rope, range: TextRange) -> TextSummary {
        let start = range.start().get();
        let end = range.end().get();
        if start == end {
            return TextSummary::default();
        }

        let (start_line_start, start_line_end) = self.line_bounds(start);
        if end <= start_line_end {
            return measure_rope(rope, start..end);
        }

        let first_full_start = if start > start_line_start {
            start_line_end
        } else {
            start_line_start
        };
        // 文本末端本身就是行边界：整段尾巴由子树摘要给出，不重测最后一行。
        let (end_line_start, _) = if end == rope.len_bytes() {
            (end, end)
        } else {
            self.line_bounds(end)
        };

        let mut summary = if first_full_start > start {
            measure_rope(rope, start..first_full_start)
        } else {
            TextSummary::default()
        };
        summary += self.range_summary(first_full_start..end_line_start);
        if end > end_line_start {
            summary += measure_rope(rope, end_line_start..end);
        }
        summary
    }

    /// 一次编辑覆盖的旧行字节区间：从包含 `range.start` 的行首到以 `range.end` 为终点的行尾。
    ///
    /// 端点恰好落在行边界时，区间可能收缩为空；调用方据此在原位置直接插入新行。
    pub(crate) fn affected_span(&self, range: TextRange) -> Range<usize> {
        let (first_start, _) = self.line_bounds(range.start().get());
        let (_, last_end) = self.line_end_bound(range.end().get());
        first_start..last_end
    }

    /// 用 `replacement` 替换 `old_span` 对应的旧行。
    ///
    /// `replacement` 是替换后文本从 `old_span.start` 起、以另一个行边界结束的字符区间；
    /// 该区间以行边界开始并以行边界结束，因此测量结果正好顶替被移除的行。
    pub(crate) fn replace_span(
        &mut self,
        old_span: Range<usize>,
        replacement: impl Iterator<Item = char>,
    ) {
        let replacement = measure_lines(replacement);
        let mut next = SumTree::new(());
        {
            let mut cursor = self.lines.cursor::<LineBytes>(());
            next.append(cursor.slice(&LineBytes(old_span.start), Bias::Right), ());
            cursor.slice(&LineBytes(old_span.end), Bias::Right);
            for line in replacement {
                next.push(line, ());
            }
            next.append(cursor.suffix(), ());
        }
        self.lines = next;
    }

    /// 完整行的摘要区间；端点必须是行边界。
    fn range_summary(&self, range: Range<usize>) -> TextSummary {
        if range.start >= range.end {
            return TextSummary::default();
        }
        let mut cursor = self.lines.cursor::<LineBytes>(());
        cursor.seek(&LineBytes(range.start), Bias::Right);
        cursor.summary(&LineBytes(range.end), Bias::Right)
    }

    /// `offset` 所在行的 `(行首, 行尾)` 字节偏移；`offset == 文本长度` 时归最后一行。
    ///
    /// `Bias::Right` 让行边界归属后一行，插入点因此落在新行的行首。
    fn line_bounds(&self, offset: usize) -> (usize, usize) {
        if self.lines.is_empty() {
            return (0, 0);
        }
        let mut cursor = self.lines.cursor::<LineBytes>(());
        cursor.seek(&LineBytes(offset), Bias::Right);
        if cursor.item().is_none() {
            cursor.prev();
        }
        (cursor.start().0, cursor.end().0)
    }

    /// 以 `offset` 为终点的行的 `(行首, 行尾)` 字节偏移。
    ///
    /// `Bias::Left` 让行边界归属前一行，编辑终点因此不会多覆盖后一行。
    fn line_end_bound(&self, offset: usize) -> (usize, usize) {
        if self.lines.is_empty() {
            return (0, 0);
        }
        let mut cursor = self.lines.cursor::<LineBytes>(());
        cursor.seek(&LineBytes(offset), Bias::Left);
        (cursor.start().0, cursor.end().0)
    }
}

/// 按换行符切分文本并测量每一行；末尾空行不保留。
fn measure_lines(chars: impl Iterator<Item = char>) -> Vec<LineSummary> {
    let mut lines = Vec::new();
    let mut current = LineSummary::default();
    for character in chars {
        current.push_char(character);
        if character == '\n' {
            lines.push(current);
            current = LineSummary::default();
        }
    }
    if current.len > 0 {
        lines.push(current);
    }
    lines
}

/// 只测量给定字节区间，不借助行树；用于区间首尾的不完整行。
fn measure_rope(rope: &Rope, range: Range<usize>) -> TextSummary {
    let slice = rope.byte_slice(range);
    TextSummary::from_chars(slice.chars(), slice.len_bytes())
}
