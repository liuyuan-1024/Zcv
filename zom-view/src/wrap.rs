//! 软换行视觉行模型。
//!
//! 这是 view 自己的概念：buffer / engine 不关心「视觉行」的存在。
//! view 把逻辑行按视口宽度切成多个视觉段，并提供围绕这套模型的查询算子。
//!
//! 三类内容内聚在一个模块里：
//!
//! - 数据：[`WrapMap`] 是「逻辑行 → 行内字节断点列表」的薄数据；
//!   [`VisualPosition`] 是 caret 的视觉投影；[`VisualAffinity`] 区分软换行边界两侧的归属。
//! - 文本域查询：[`WrapMap::resolve`] / [`WrapMap::step_visual_row`] / [`WrapMap::visual_line_edge`] / [`WrapMap::grapheme`]。
//! command 层只在文本域查询；不需要 shape、不需要等下一帧。
//! - 分段算法：[`compute_segments`] 把一条逻辑行按视口宽度切成 sub-row 字节区间；
//! 合法断点策略（空白后、CJK 边界等）一并在内部 helper [`is_cjk_break_candidate`] 中闭合。
//! 渲染端只把字形宽度（`x_for_index`）以闭包形式喂进来，不自己实现切分。
//!
//! Caret sticky 用「视觉列」（grapheme 数）。
//! 变宽字体下垂直移动按列对齐而非像素对齐——这是有意取舍：
//! 模型在文本域闭合，无需 shape 反馈，与帧渲染节奏完全解耦。
//!
//! 不变量：
//! - `breaks_per_line[L]` 的断点是行内相对字节，单调递增，落在 grapheme 边界上。
//! - 不开软换行时所有行的 breaks 列表为空——每条逻辑行恰好 1 个 subrow，查询算子的行为退化为「按逻辑行」，无需在调用端再走分支。
//! - 断点不含 0 和行尾，故 `subrow_count(line) = breaks(line).len() + 1`。

use zom_engine::{Buffer, ByteOffset, EngineResult, Line, MovementDirection};

/// 同一个 byte 在软换行边界处可能对应两个视觉位置；affinity 用于区分。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VisualAffinity {
    /// 视觉行起点。软换行边界 byte 归属下一视觉行行首时使用。
    LineStart,
    /// 视觉行内部，非边界位置。
    Inside,
    /// 视觉行末尾。软换行边界 byte 归属上一视觉行行尾时使用。
    LineEnd,
}

/// 视觉光标位置：byte + 视觉坐标 + 边界归属。
///
/// `column` 用 grapheme 数自视觉行起点起算；视觉行末尾位置 column == grapheme_count。
/// `subrow` 是逻辑行内的视觉段序号（0-based）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VisualPosition {
    pub byte: ByteOffset,
    pub logical_line: u64,
    pub subrow: u32,
    pub column: u32,
    pub affinity: VisualAffinity,
}

/// 每条逻辑行的软换行断点（行内相对字节）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WrapMap {
    soft_wrap: bool,
    breaks_per_line: Vec<Vec<u32>>,
}

impl WrapMap {
    /// 构造一份新的 WrapMap。
    ///
    /// `breaks_per_line[i]` 必须为单调递增、不含 0 与行尾、落在 grapheme 边界的相对字节列表；
    /// 不软换行时传入与逻辑行数相同长度的空向量。
    pub fn new(soft_wrap: bool, breaks_per_line: Vec<Vec<u32>>) -> Self {
        Self {
            soft_wrap,
            breaks_per_line,
        }
    }

    pub fn soft_wrap(&self) -> bool {
        self.soft_wrap
    }

    pub fn logical_line_count(&self) -> u64 {
        self.breaks_per_line.len() as u64
    }

    /// 指定逻辑行的断点列表（行内相对字节）。越界返回空 slice。
    pub fn breaks(&self, line: u64) -> &[u32] {
        self.breaks_per_line
            .get(line as usize)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// 指定逻辑行的视觉段数。越界（理论上不会发生）退化为 1。
    pub fn subrow_count(&self, line: u64) -> u32 {
        self.breaks_per_line
            .get(line as usize)
            .map(|v| v.len() as u32 + 1)
            .unwrap_or(1)
    }

    /// 视觉行总数；用于 viewport / scroll 行号换算。
    pub fn total_visual_rows(&self) -> u64 {
        self.breaks_per_line
            .iter()
            .map(|v| v.len() as u64 + 1)
            .sum()
    }

    /// 把 byte 解析成 VisualPosition。
    ///
    /// 软换行边界（即 byte 恰好等于某个断点的绝对位置）存在两种视觉位置；
    /// 由 `hint` 决定取上一段行尾还是下一段行首。`hint == None` 时默认下一段行首
    /// （与渲染端历史行为一致）。
    pub fn resolve(
        &self,
        buffer: &Buffer,
        byte: ByteOffset,
        hint: Option<VisualAffinity>,
    ) -> EngineResult<VisualPosition> {
        let line = buffer.byte_to_line(byte)?;
        let line_start = buffer.line_start_byte(line)?;
        let relative = (byte.get() - line_start.get()) as u32;
        let logical_line = line_index_u64(line);
        let breaks = self.breaks(logical_line);

        // 找出 byte 落在哪个 subrow：
        // breaks 按行内相对字节升序。subrow K 的范围 = [breaks[K-1] (或 0), breaks[K] (或 line_len)]。
        let (subrow, sub_start_rel) = locate_subrow(breaks, relative, hint);

        // 列 = subrow 内从起点到 byte 的 grapheme 数。
        let sub_start = ByteOffset::new(line_start.get() + sub_start_rel as usize);
        let column = count_graphemes_between(buffer, sub_start, byte)?;

        let affinity = resolve_affinity(breaks, relative, subrow, hint);

        Ok(VisualPosition {
            byte,
            logical_line,
            subrow,
            column,
            affinity,
        })
    }

    /// 垂直移动 `n` 视觉行；按 `goal_column` 在新视觉行上取等列或最近列。
    ///
    /// - 向上跨过首行 → 文档开头（line 0, subrow 0, column 0）。
    /// - 向下跨过末行末段 → 文档末尾（最后一行最后 subrow 的行尾）。
    /// - `goal_column` 超过目标视觉行可用列数时夹到行尾。
    pub fn step_visual_row(
        &self,
        buffer: &Buffer,
        pos: VisualPosition,
        dir: MovementDirection,
        n: u32,
        goal_column: u32,
    ) -> EngineResult<VisualPosition> {
        if n == 0 {
            return Ok(pos);
        }
        let (target_line, target_subrow) = match dir {
            MovementDirection::Previous => self.step_subrow_back(pos.logical_line, pos.subrow, n),
            MovementDirection::Next => self.step_subrow_forward(pos.logical_line, pos.subrow, n),
        };

        // 如果目标 = 起点（已经在边缘），仍然把 caret 摆到该视觉行起点 / 末尾，
        // 与 engine 既有 LineStep 在文档边缘的行为对齐。
        let at_doc_start = matches!(dir, MovementDirection::Previous)
            && target_line == pos.logical_line
            && target_subrow == pos.subrow
            && pos.subrow == 0
            && pos.logical_line == 0;
        let at_doc_end = matches!(dir, MovementDirection::Next)
            && target_line == pos.logical_line
            && target_subrow == pos.subrow
            && pos.subrow + 1 == self.subrow_count(pos.logical_line)
            && pos.logical_line + 1 == self.logical_line_count();

        self.subrow_position(buffer, target_line, target_subrow, |sub_grapheme_count| {
            if at_doc_start {
                0
            } else if at_doc_end {
                sub_grapheme_count
            } else {
                goal_column.min(sub_grapheme_count)
            }
        })
    }

    /// 视觉行首 / 行尾。
    pub fn visual_line_edge(
        &self,
        buffer: &Buffer,
        pos: VisualPosition,
        dir: MovementDirection,
    ) -> EngineResult<VisualPosition> {
        self.subrow_position(
            buffer,
            pos.logical_line,
            pos.subrow,
            |sub_grapheme_count| match dir {
                MovementDirection::Previous => 0,
                MovementDirection::Next => sub_grapheme_count,
            },
        )
    }

    /// 单 grapheme 步进；在 wrap 边界处可以「原地」跨段（byte 不变，affinity 翻转）。
    ///
    /// 返回 `None` 表示已在文档边缘无法再移动。
    pub fn grapheme(
        &self,
        buffer: &Buffer,
        pos: VisualPosition,
        dir: MovementDirection,
    ) -> EngineResult<Option<VisualPosition>> {
        match dir {
            MovementDirection::Previous => {
                // 在 LineStart 且不是逻辑行第一段：跨到上一段 LineEnd，byte 不变。
                if pos.affinity == VisualAffinity::LineStart && pos.subrow > 0 {
                    let upper = pos.subrow - 1;
                    let pos = self.subrow_position(buffer, pos.logical_line, upper, |gc| gc)?;
                    return Ok(Some(pos));
                }
                // 否则物理前移一格 grapheme。
                if pos.byte.get() == 0 {
                    return Ok(None);
                }
                let prev = buffer.previous_grapheme_boundary_byte(pos.byte)?;
                Ok(Some(self.resolve(buffer, prev, None)?))
            }
            MovementDirection::Next => {
                // 在 LineEnd 且不是逻辑行最后一段：跨到下一段 LineStart，byte 不变。
                if pos.affinity == VisualAffinity::LineEnd
                    && pos.subrow + 1 < self.subrow_count(pos.logical_line)
                {
                    let lower = pos.subrow + 1;
                    let pos = self.subrow_position(buffer, pos.logical_line, lower, |_| 0)?;
                    return Ok(Some(pos));
                }
                let next = buffer.next_grapheme_boundary_byte(pos.byte)?;
                if next == pos.byte {
                    return Ok(None);
                }
                Ok(Some(self.resolve(buffer, next, None)?))
            }
        }
    }
}

impl WrapMap {
    fn step_subrow_back(&self, mut line: u64, mut subrow: u32, mut n: u32) -> (u64, u32) {
        while n > 0 {
            if subrow > 0 {
                subrow -= 1;
                n -= 1;
                continue;
            }
            if line == 0 {
                return (0, 0);
            }
            line -= 1;
            subrow = self.subrow_count(line).saturating_sub(1);
            n -= 1;
        }
        (line, subrow)
    }

    fn step_subrow_forward(&self, mut line: u64, mut subrow: u32, mut n: u32) -> (u64, u32) {
        let total_lines = self.logical_line_count();
        while n > 0 {
            let count = self.subrow_count(line);
            if subrow + 1 < count {
                subrow += 1;
                n -= 1;
                continue;
            }
            if line + 1 >= total_lines {
                return (line, count.saturating_sub(1));
            }
            line += 1;
            subrow = 0;
            n -= 1;
        }
        (line, subrow)
    }

    /// 计算指定 (line, subrow) 的视觉行起止区间（行内相对字节）。
    fn subrow_relative_range(&self, line: u64, subrow: u32) -> (u32, Option<u32>) {
        let breaks = self.breaks(line);
        let start = if subrow == 0 {
            0
        } else {
            breaks[(subrow - 1) as usize]
        };
        let end = breaks.get(subrow as usize).copied();
        (start, end)
    }

    /// 构造 (line, subrow) 视觉段的 VisualPosition，列位由 `pick` 在 [0, grapheme_count] 内选定。
    fn subrow_position(
        &self,
        buffer: &Buffer,
        line: u64,
        subrow: u32,
        pick: impl FnOnce(u32) -> u32,
    ) -> EngineResult<VisualPosition> {
        let line_start = buffer.line_start_byte(line_from_u64(line))?;
        let (start_rel, end_rel) = self.subrow_relative_range(line, subrow);
        let sub_start = ByteOffset::new(line_start.get() + start_rel as usize);
        let sub_end_byte = match end_rel {
            Some(end) => ByteOffset::new(line_start.get() + end as usize),
            None => line_end_byte(buffer, line)?,
        };
        let grapheme_count = count_graphemes_between(buffer, sub_start, sub_end_byte)?;
        let column = pick(grapheme_count);
        let byte = byte_at_grapheme_offset(buffer, sub_start, sub_end_byte, column)?;
        let affinity = if column == 0 && subrow > 0 {
            VisualAffinity::LineStart
        } else if column == grapheme_count && end_rel.is_some() {
            VisualAffinity::LineEnd
        } else {
            VisualAffinity::Inside
        };
        Ok(VisualPosition {
            byte,
            logical_line: line,
            subrow,
            column,
            affinity,
        })
    }
}

/// 行号转 `Line`。
fn line_from_u64(line: u64) -> Line {
    Line::new(line as usize)
}

fn line_index_u64(line: Line) -> u64 {
    line.get() as u64
}

/// 指定逻辑行的末尾字节（不含换行符）。
///
/// 末尾 = 下一行起点 - 该行换行符字节数。最后一行 = buffer 总长度。
fn line_end_byte(buffer: &Buffer, line: u64) -> EngineResult<ByteOffset> {
    let total_lines = buffer.line_count() as u64;
    if line + 1 >= total_lines {
        // 末行：buffer 末尾。
        let last = Line::new(buffer.line_count().saturating_sub(1));
        let start = buffer.line_start_byte(last)?;
        // 走 byte_to_position 取行长太重；这里靠 next_grapheme_boundary 一直走到行末不可行。
        // 直接读 buffer 总字节数：通过最后一行起点 + 切片长度。
        let slice = buffer.slice_line(last)?;
        return Ok(ByteOffset::new(start.get() + slice.len_bytes()));
    }
    // 非末行：下一行起点回退一个 grapheme = 当前行 \n 之前。
    let next_start = buffer.line_start_byte(line_from_u64(line + 1))?;
    let end = buffer.previous_grapheme_boundary_byte(next_start)?;
    Ok(end)
}

fn count_graphemes_between(buffer: &Buffer, from: ByteOffset, to: ByteOffset) -> EngineResult<u32> {
    debug_assert!(from <= to);
    let mut n: u32 = 0;
    let mut cursor = from;
    while cursor < to {
        let next = buffer.next_grapheme_boundary_byte(cursor)?;
        if next <= cursor {
            break;
        }
        cursor = next;
        n += 1;
    }
    Ok(n)
}

fn byte_at_grapheme_offset(
    buffer: &Buffer,
    start: ByteOffset,
    end: ByteOffset,
    column: u32,
) -> EngineResult<ByteOffset> {
    let mut cursor = start;
    for _ in 0..column {
        if cursor >= end {
            break;
        }
        let next = buffer.next_grapheme_boundary_byte(cursor)?;
        if next <= cursor || next > end {
            break;
        }
        cursor = next;
    }
    Ok(cursor)
}

fn locate_subrow(breaks: &[u32], relative: u32, hint: Option<VisualAffinity>) -> (u32, u32) {
    if breaks.is_empty() {
        return (0, 0);
    }
    match breaks.binary_search(&relative) {
        Ok(idx) => {
            // 命中断点：byte 同时是第 idx 段末尾与第 idx+1 段开头。
            let prefer_end = matches!(hint, Some(VisualAffinity::LineEnd));
            if prefer_end {
                let subrow = idx as u32;
                let start = if subrow == 0 {
                    0
                } else {
                    breaks[(subrow - 1) as usize]
                };
                (subrow, start)
            } else {
                let subrow = idx as u32 + 1;
                (subrow, breaks[idx])
            }
        }
        Err(idx) => {
            // 落在第 idx 段内部。
            let subrow = idx as u32;
            let start = if idx == 0 { 0 } else { breaks[idx - 1] };
            (subrow, start)
        }
    }
}

fn resolve_affinity(
    breaks: &[u32],
    relative: u32,
    subrow: u32,
    hint: Option<VisualAffinity>,
) -> VisualAffinity {
    // 命中某条断点字节：上段末尾或下段开头。
    if breaks.binary_search(&relative).is_ok() {
        return match hint {
            Some(VisualAffinity::LineEnd) => VisualAffinity::LineEnd,
            _ => VisualAffinity::LineStart,
        };
    }
    // subrow 内部位置：当 relative == subrow 起点（且非首段）算 LineStart；
    // 当 relative == subrow 末尾算 LineEnd（仅当存在断点边界，否则是逻辑行末，归为 Inside）。
    let start = if subrow == 0 {
        0
    } else {
        breaks[(subrow - 1) as usize]
    };
    let end = breaks.get(subrow as usize).copied();
    if relative == start && subrow > 0 {
        VisualAffinity::LineStart
    } else if Some(relative) == end {
        VisualAffinity::LineEnd
    } else {
        VisualAffinity::Inside
    }
}

/// 软换行 MVP 断行算法。
///
/// 按字节遍历 `text`，用 `measure(byte)` 取从行首到 `byte`（必须落在字符边界）的累计像素位置；
/// 当 sub-row 宽度即将超过 `viewport_width` 时，回退到最近的「合法断点」断；
/// 找不到就在当前字符位置硬断。
///
/// `total_width = measure(text.len())`，用于一开始判断「整行能不能放下」并提前返回；
/// 调用方通常已经为别的用途 shape 过这条行，复用这个值能省一次测量。
///
/// **合法断点**有两种（参考 UAX #14 的实用子集）：
/// - **空白后**：任何空白字符之后的位置——空白会留在前一 sub-row 末尾。
/// - **CJK 边界**：CJK 字符与任何字符之间的位置（CJK-CJK / CJK-ASCII / ASCII-CJK）。
///   CJK 之间没有空白也要能断，否则一长串中文遇到行尾会被整段挤到下一行，
///   留出大块空白——视觉上「明明右边还有位置为啥换了」的来源。
///
/// 返回 sub-row 的字节区间列表 `[(start, end), ...]`，覆盖 `[0, text.len())`，
/// 互不重叠且按顺序排列；不开软换行 / 整行能放下 / `viewport_width <= 0` 时
/// 直接返回 `[(0, text.len())]`。
///
/// 不变量：
/// - 单个字符比 `viewport_width` 还宽时，仍把这个字符放入当前 sub-row（避免空段死循环）；
///   下一个字符再触发断行。
/// - 断点字节总落在 UTF-8 字符边界——本函数只在字符边界处记录断点。
/// - `text == ""` 返回 `[(0, 0)]`，与上游 TextRun 切分对空行的约定一致。
pub fn compute_segments(
    text: &str,
    total_width: f32,
    viewport_width: f32,
    measure: impl Fn(usize) -> f32,
) -> Vec<(usize, usize)> {
    if text.is_empty() {
        return vec![(0, 0)];
    }
    if viewport_width <= 0.0 {
        return vec![(0, text.len())];
    }
    if total_width <= viewport_width {
        return vec![(0, text.len())];
    }

    let mut segments: Vec<(usize, usize)> = Vec::new();
    let mut current_start: usize = 0;
    let mut current_start_x = measure(0);
    // 最近一个「合法断点」的字节位置——溢出时把它当成 sub-row 切点。
    // 空白后的合法断点 = 空白字符之后的字节；CJK 边界的合法断点 = 当前字节。
    let mut last_break: Option<usize> = None;
    // 前一字符是不是 CJK——用于判定「CJK / 非-CJK」切换处也是断点。
    let mut prev_was_cjk = false;

    let bytes = text.as_bytes();
    let mut byte: usize = 0;
    while byte < text.len() {
        // 推进到下一个 UTF-8 字符边界。
        let mut next = byte + 1;
        while next < text.len() && (bytes[next] & 0xC0) == 0x80 {
            next += 1;
        }

        let ch = text[byte..next].chars().next().unwrap_or(' ');
        let curr_is_cjk = is_cjk_break_candidate(ch);

        if curr_is_cjk || prev_was_cjk {
            last_break = Some(byte);
        }

        let x_next = measure(next);
        let segment_w = x_next - current_start_x;

        if segment_w > viewport_width && byte > current_start {
            let break_at = match last_break {
                Some(p) if p > current_start && p <= byte => p,
                _ => byte,
            };
            segments.push((current_start, break_at));
            current_start = break_at;
            current_start_x = measure(break_at);
            last_break = None;
            prev_was_cjk = false;
            // 不推进 byte——本字符重新进入下一 sub-row。
            continue;
        }

        if ch.is_whitespace() {
            last_break = Some(next);
        }

        prev_was_cjk = curr_is_cjk;
        byte = next;
    }

    if current_start < text.len() {
        segments.push((current_start, text.len()));
    }
    if segments.is_empty() {
        segments.push((0, text.len()));
    }
    segments
}

/// 是否是「东亚正文字符」——把它两侧都视为软换行的合法断点。
///
/// 覆盖：
/// - CJK 符号与标点（U+3000-303F）：含全角空格、句号、逗号等。
///   注意句末连续标点其实有 UAX #14 的「不可前断 / 不可后断」分类，MVP 一并归入可断；
///   极端情况下会出现「，」开头的续行，可接受。
/// - 平假名、片假名、CJK 偏旁、康熙部首、注音符号、CJK 基本与扩展 A、CJK 兼容、全角形式。
/// - 韩文音节（U+AC00-D7A3）。
/// - 扩展 B/C/D/E/F 落在 SMP，照样命中。
///
/// 不包含拉丁、阿拉伯数字、ASCII 标点——这些走「空白后」断点路径。
fn is_cjk_break_candidate(c: char) -> bool {
    let code = c as u32;
    matches!(
        code,
        0x2E80..=0x2EFF   // CJK 偏旁补充
        | 0x2F00..=0x2FDF // 康熙部首
        | 0x3000..=0x303F // CJK 符号与标点
        | 0x3040..=0x309F // 平假名
        | 0x30A0..=0x30FF // 片假名
        | 0x3100..=0x312F // 注音符号
        | 0x3130..=0x318F // 谚文兼容字母
        | 0x3190..=0x319F // 汉文训点
        | 0x31A0..=0x31BF // 注音符号扩展
        | 0x31C0..=0x31EF // CJK 笔画
        | 0x31F0..=0x31FF // 片假名拼音扩展
        | 0x3200..=0x32FF // 带圈 CJK 字母月
        | 0x3300..=0x33FF // CJK 兼容
        | 0x3400..=0x4DBF // CJK 扩展 A
        | 0x4E00..=0x9FFF // CJK 基础
        | 0xA000..=0xA48F // 彝文
        | 0xAC00..=0xD7A3 // 谚文音节
        | 0xF900..=0xFAFF // CJK 兼容表意文字
        | 0xFE30..=0xFE4F // CJK 兼容形式
        | 0xFF00..=0xFFEF // 全角 / 半角形式
        | 0x20000..=0x2EBEF // CJK 扩展 B/C/D/E/F
        | 0x2F800..=0x2FA1F // CJK 兼容表意文字补充
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use zom_engine::BufferConfig;

    fn buf(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    /// 整篇 "abcdefghi" 按宽度切成三段：[0,3) [3,6) [6,9)。
    fn map_3_3_3() -> WrapMap {
        WrapMap::new(true, vec![vec![3, 6]])
    }

    #[test]
    fn resolve_inside_subrow_returns_inside_affinity() {
        let buffer = buf("abcdefghi");
        let wm = map_3_3_3();
        let p = wm.resolve(&buffer, ByteOffset::new(4), None).unwrap();
        assert_eq!(p.subrow, 1);
        assert_eq!(p.column, 1);
        assert_eq!(p.affinity, VisualAffinity::Inside);
    }

    #[test]
    fn resolve_break_byte_defaults_to_next_line_start() {
        let buffer = buf("abcdefghi");
        let wm = map_3_3_3();
        let p = wm.resolve(&buffer, ByteOffset::new(3), None).unwrap();
        assert_eq!(p.subrow, 1);
        assert_eq!(p.column, 0);
        assert_eq!(p.affinity, VisualAffinity::LineStart);
    }

    #[test]
    fn resolve_break_byte_with_line_end_hint_picks_upper_subrow() {
        let buffer = buf("abcdefghi");
        let wm = map_3_3_3();
        let p = wm
            .resolve(&buffer, ByteOffset::new(3), Some(VisualAffinity::LineEnd))
            .unwrap();
        assert_eq!(p.subrow, 0);
        assert_eq!(p.column, 3);
        assert_eq!(p.affinity, VisualAffinity::LineEnd);
    }

    #[test]
    fn step_visual_row_down_carries_column_within_logical_line() {
        let buffer = buf("abcdefghi");
        let wm = map_3_3_3();
        let start = wm.resolve(&buffer, ByteOffset::new(1), None).unwrap();
        let down = wm
            .step_visual_row(&buffer, start, MovementDirection::Next, 1, start.column)
            .unwrap();
        assert_eq!(down.subrow, 1);
        assert_eq!(down.column, 1);
        assert_eq!(down.byte, ByteOffset::new(4));
    }

    #[test]
    fn step_visual_row_clamps_goal_column_when_target_shorter() {
        // 行 0：'a','b','c' (3 列)；行 1（逻辑行 1）：'x' (1 列)。
        let buffer = buf("abc\nx");
        let wm = WrapMap::new(false, vec![vec![], vec![]]);
        let start = wm.resolve(&buffer, ByteOffset::new(2), None).unwrap();
        let down = wm
            .step_visual_row(&buffer, start, MovementDirection::Next, 1, 5)
            .unwrap();
        assert_eq!(down.logical_line, 1);
        assert_eq!(down.column, 1);
    }

    #[test]
    fn step_visual_row_at_doc_start_lands_on_column_zero() {
        let buffer = buf("abc");
        let wm = WrapMap::new(false, vec![vec![]]);
        let start = wm.resolve(&buffer, ByteOffset::new(2), None).unwrap();
        let up = wm
            .step_visual_row(&buffer, start, MovementDirection::Previous, 1, 2)
            .unwrap();
        assert_eq!(up.byte, ByteOffset::new(0));
        assert_eq!(up.column, 0);
    }

    #[test]
    fn step_visual_row_at_doc_end_lands_on_line_end() {
        let buffer = buf("abc");
        let wm = WrapMap::new(false, vec![vec![]]);
        let start = wm.resolve(&buffer, ByteOffset::new(1), None).unwrap();
        let down = wm
            .step_visual_row(&buffer, start, MovementDirection::Next, 1, 1)
            .unwrap();
        assert_eq!(down.byte, ByteOffset::new(3));
    }

    #[test]
    fn visual_line_edge_at_subrow_end_returns_line_end_affinity() {
        let buffer = buf("abcdefghi");
        let wm = map_3_3_3();
        let start = wm.resolve(&buffer, ByteOffset::new(4), None).unwrap();
        let end = wm
            .visual_line_edge(&buffer, start, MovementDirection::Next)
            .unwrap();
        assert_eq!(end.byte, ByteOffset::new(6));
        assert_eq!(end.affinity, VisualAffinity::LineEnd);
    }

    #[test]
    fn grapheme_next_crosses_wrap_boundary_without_moving_byte() {
        let buffer = buf("abcdefghi");
        let wm = map_3_3_3();
        // 上段末尾。
        let upper_end = wm
            .resolve(&buffer, ByteOffset::new(3), Some(VisualAffinity::LineEnd))
            .unwrap();
        let next = wm
            .grapheme(&buffer, upper_end, MovementDirection::Next)
            .unwrap()
            .unwrap();
        assert_eq!(next.byte, ByteOffset::new(3));
        assert_eq!(next.subrow, 1);
        assert_eq!(next.affinity, VisualAffinity::LineStart);
    }

    #[test]
    fn grapheme_previous_crosses_wrap_boundary_without_moving_byte() {
        let buffer = buf("abcdefghi");
        let wm = map_3_3_3();
        // 下段起点。
        let lower_start = wm.resolve(&buffer, ByteOffset::new(3), None).unwrap();
        let prev = wm
            .grapheme(&buffer, lower_start, MovementDirection::Previous)
            .unwrap()
            .unwrap();
        assert_eq!(prev.byte, ByteOffset::new(3));
        assert_eq!(prev.subrow, 0);
        assert_eq!(prev.affinity, VisualAffinity::LineEnd);
    }

    #[test]
    fn total_visual_rows_sums_subrows() {
        // 行 0 三段；行 1 一段。
        let wm = WrapMap::new(true, vec![vec![3, 6], vec![]]);
        assert_eq!(wm.total_visual_rows(), 4);
    }

    /// 等宽假设下每个字节宽度 10——便于在没有 GPUI ShapedLine 时验证 [`compute_segments`]。
    fn equal_width(byte: usize) -> f32 {
        byte as f32 * 10.0
    }

    #[test]
    fn compute_segments_returns_whole_line_when_fits() {
        let segs = compute_segments("abc", 30.0, 100.0, equal_width);
        assert_eq!(segs, vec![(0, 3)]);
    }

    #[test]
    fn compute_segments_returns_zero_zero_for_empty_line() {
        let segs = compute_segments("", 0.0, 100.0, equal_width);
        assert_eq!(segs, vec![(0, 0)]);
    }

    #[test]
    fn compute_segments_hard_breaks_when_no_legal_break() {
        // 9 个字符宽 90，视口 30 → 期望 [0,3) [3,6) [6,9)。
        let segs = compute_segments("abcdefghi", 90.0, 30.0, equal_width);
        assert_eq!(segs, vec![(0, 3), (3, 6), (6, 9)]);
    }

    #[test]
    fn compute_segments_breaks_after_whitespace() {
        // "ab cd ef" 长 80；视口 50（= 5 字节）。
        // 走到 byte 5 时 "ab cd " 的宽度溢出，触发回退到 last_break=3（空白后）。
        // 剩下 "cd ef" 长 50，正好不溢出。
        let segs = compute_segments("ab cd ef", 80.0, 50.0, equal_width);
        assert_eq!(segs, vec![(0, 3), (3, 8)]);
    }

    #[test]
    fn compute_segments_breaks_at_cjk_boundary_without_whitespace() {
        // "ab中文cd"：a(1) b(1) 中(3) 文(3) c(1) d(1) = 10 bytes。等宽函数按字节算，
        // 视口 50 = 5 字节宽，应在 CJK 边界处断。
        let text = "ab中文cd";
        // 字节宽度：[0,1,2,5,8,9,10]。
        // 验证断点至少落在 CJK 字符边界上：byte 2 (b 之后，中 之前) 或 byte 8 (文 之后)。
        let segs = compute_segments(text, equal_width(text.len()), 50.0, equal_width);
        assert!(segs.len() >= 2, "should wrap at CJK boundary: {:?}", segs);
        let break_at = segs[0].1;
        assert!(
            break_at == 2 || break_at == 5 || break_at == 8,
            "break {} should land at a CJK boundary (2/5/8)",
            break_at
        );
    }
}
