//! DisplayMap 的 Tab 展开与 display-column 映射。
//!
//! `TabMap` 只测量实际进入投影视口的逻辑行，并在同行编辑后精确失效对应缓存。
//! 初次构建不遍历全文；结构编辑按行区间平移已测量行（被编辑行失效），后续仍按需重新填充。

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use zcv_multi_buffer::MultiBufferSnapshot;
use zcv_text::{BufferConfig, ByteOffset, CoordinateError, Line};

use super::chunk::{ChunkBase, ChunkText, FoldChunks, HighlightStyles, InlayChunks};
use super::display_width::{DisplayColumn, char_width};
use super::{
    error::DisplayMapResult,
    fold_map::{FoldEdit, FoldSnapshot, ProjectedLineIndex, StreamProjectedKind},
    line_stream::LineStream,
};

#[derive(Debug, Clone)]
pub(crate) struct TabSnapshot {
    fold_snapshot: FoldSnapshot,
    version: u64,
}

impl TabSnapshot {
    pub(super) fn new(fold_snapshot: FoldSnapshot) -> Self {
        Self {
            fold_snapshot,
            version: 0,
        }
    }

    pub(crate) fn stream(&self) -> &LineStream {
        self.fold_snapshot.stream()
    }

    pub(super) fn buffer_snapshot(&self) -> &MultiBufferSnapshot {
        self.fold_snapshot.buffer_snapshot()
    }

    pub(crate) fn fold_snapshot(&self) -> &FoldSnapshot {
        &self.fold_snapshot
    }

    /// 投影行总数（fold 输出）。
    pub(super) fn line_count(&self) -> usize {
        self.fold_snapshot.line_count()
    }

    /// 投影行 → 对应的 buffer 行来源。
    pub(super) fn projected_kind(&self, line: Line) -> Option<StreamProjectedKind> {
        self.fold_snapshot
            .projected_kind(ProjectedLineIndex::new(line.get()))
    }

    /// 投影行 → 行文本（经流行解析与行内提示注入；折叠合并行为合成文本）。
    pub(super) fn line_text(&self, line: Line) -> Option<Cow<'_, str>> {
        let fold = self.fold_snapshot();
        let projected = ProjectedLineIndex::new(line.get());
        if fold.is_fold_row(projected) {
            return fold.row_text(projected);
        }
        let inlay = fold.inlay_snapshot();
        let stream_line = self.stream_line_for_projected(line)?;
        inlay.line_text(stream_line)
    }

    /// 投影行 → 字节范围（折叠合并行为锚定行行首的伪坐标）。
    pub(super) fn line_byte_range(&self, line: Line) -> Option<Range<ByteOffset>> {
        let fold = self.fold_snapshot();
        let projected = ProjectedLineIndex::new(line.get());
        if let Some(anchor_stream) = fold.fold_row_anchor_stream_line(projected) {
            // 合并行：anchor 行行首的伪坐标，roundtrip 不可逆。
            let buffer_line = fold.inlay_snapshot().source(anchor_stream)?.line();
            let start = fold
                .buffer_snapshot()
                .line_start_byte(Line::new(buffer_line))
                .ok()?;
            return Some(start..start);
        }
        let inlay = fold.inlay_snapshot();
        let stream_line = self.stream_line_for_projected(line)?;
        inlay.line_byte_range(stream_line)
    }

    /// 投影行 → 流行号（坐标换算用）。
    pub(super) fn stream_line_for_projected(&self, line: Line) -> Option<Line> {
        let inlay = self.fold_snapshot.inlay_snapshot();
        match self.projected_kind(line)? {
            StreamProjectedKind::Text(source) => {
                Some(inlay.stream().buffer_to_stream(Line::new(source.line())))
            }
        }
    }

    pub(super) const fn version(&self) -> u64 {
        self.version
    }
}

#[derive(Debug, Clone)]
pub(super) struct TabMap {
    snapshot: TabSnapshot,
    measured_line_widths: BTreeMap<Line, DisplayColumn>,
    longest_measured: Option<(Line, DisplayColumn)>,
}

impl TabMap {
    pub(super) fn new(fold_snapshot: FoldSnapshot) -> (Self, TabSnapshot) {
        let snapshot = TabSnapshot::new(fold_snapshot);
        (
            Self {
                snapshot: snapshot.clone(),
                measured_line_widths: BTreeMap::new(),
                longest_measured: None,
            },
            snapshot,
        )
    }

    pub(super) fn sync(
        &mut self,
        fold_snapshot: FoldSnapshot,
        fold_edits: &[FoldEdit],
    ) -> TabSnapshot {
        let snapshot = fold_snapshot.buffer_snapshot();
        let previous_snapshot = self.snapshot.buffer_snapshot();
        // display 策略随 BufferConfig 移除，缓存失效只以 tab 配置变化为键。
        let same_configuration = previous_snapshot.config().tab == snapshot.config().tab;
        // fold 拓扑（折叠/行内提示变化都会使 fold 版本前进）。
        let same_fold_version = self.snapshot.fold_snapshot.version() == fold_snapshot.version();

        if same_configuration && same_fold_version {
            self.snapshot = TabSnapshot {
                fold_snapshot,
                version: self.snapshot.version,
            };
            return self.snapshot.clone();
        }

        let new_version = self.snapshot.version + 1;
        // 结构编辑按 FoldEdit 的旧/新行区间平移宽度缓存；
        // 行内编辑按 changed_lines 精确失效。
        let structural = fold_edits.iter().any(FoldEdit::is_structural);
        if !same_configuration {
            self.measured_line_widths.clear();
            self.longest_measured = None;
        } else if structural {
            for edit in fold_edits.iter().filter(|edit| edit.is_structural()) {
                self.shift_measured_widths(edit.old_rows(), edit.new_rows());
            }
            self.longest_measured = self
                .measured_line_widths
                .iter()
                .max_by_key(|(_, width)| **width)
                .map(|(line, width)| (*line, *width));
        } else {
            let mut changed_lines = BTreeSet::new();
            for edit in fold_edits {
                changed_lines.extend(edit.changed_lines().iter().copied());
            }
            self.measured_line_widths
                .retain(|line, _| !changed_lines.contains(line));
            if self
                .longest_measured
                .is_some_and(|(line, _)| changed_lines.contains(&line))
            {
                self.longest_measured = self
                    .measured_line_widths
                    .iter()
                    .max_by_key(|(_, width)| **width)
                    .map(|(line, width)| (*line, *width));
            }
        }
        self.snapshot = TabSnapshot {
            fold_snapshot,
            version: new_version,
        };
        self.snapshot.clone()
    }

    /// 结构编辑后平移宽度缓存：旧行区间内的键失效，其后的键按行数差整体平移。
    ///
    /// 折叠覆盖行仍映射到其 anchor 行的合并行；被编辑的合并行落在旧行区间内，因而被丢弃。
    fn shift_measured_widths(&mut self, old_rows: Range<usize>, new_rows: Range<usize>) {
        debug_assert_eq!(
            old_rows.start, new_rows.start,
            "结构编辑的旧/新行区间必须共享起点，才能平移未受影响的缓存"
        );
        let delta = new_rows.len() as isize - old_rows.len() as isize;
        let widths = std::mem::take(&mut self.measured_line_widths);
        for (line, width) in widths {
            let row = line.get();
            if row < old_rows.start {
                self.measured_line_widths.insert(line, width);
            } else if row >= old_rows.end {
                self.measured_line_widths
                    .insert(Line::new((row as isize + delta) as usize), width);
            }
        }
    }

    pub(super) fn measure_line(&mut self, line: Line) -> DisplayMapResult<DisplayColumn> {
        if let Some(width) = self.measured_line_widths.get(&line) {
            return Ok(*width);
        }
        let snapshot = self.snapshot.stream().buffer_snapshot();
        let fold = self.snapshot.fold_snapshot();
        let projected = ProjectedLineIndex::new(line.get());
        let mut width = 0;
        if let Some(segments) = fold.fold_row_segments(projected) {
            let content_len = segments
                .last()
                .expect("折叠合并行必须至少包含一个段")
                .merged_range()
                .end;
            for chunk in FoldChunks::new(
                &segments,
                fold.inlay_snapshot(),
                HighlightStyles::default(),
                0..content_len,
            ) {
                width = display_width_chunk(width, chunk.text, snapshot.config());
            }
        } else {
            let stream_line = self
                .snapshot
                .stream_line_for_projected(line)
                .ok_or(CoordinateError::LineOutOfBounds(line))?;
            let inlay = fold.inlay_snapshot();
            let range = inlay
                .line_content_byte_range(stream_line)
                .ok_or(CoordinateError::LineOutOfBounds(line))?;
            let content_len = inlay
                .projected_line_content_metrics(stream_line)
                .ok_or(CoordinateError::LineOutOfBounds(line))?
                .0;
            for chunk in InlayChunks::new(
                ChunkText::Virtual {
                    snapshot: inlay.buffer_snapshot(),
                    range: range.clone(),
                },
                range.start.get(),
                inlay.line_inlays(stream_line),
                ChunkBase::ZERO,
                HighlightStyles::default(),
                0..content_len,
                true,
            ) {
                width = display_width_chunk(width, chunk.text, snapshot.config());
            }
        }
        let width = DisplayColumn::new(width);
        self.measured_line_widths.insert(line, width);
        if self
            .longest_measured
            .is_none_or(|(_, longest)| width > longest)
        {
            self.longest_measured = Some((line, width));
        }
        Ok(width)
    }

    pub(super) fn longest_measured(&self) -> Option<(Line, DisplayColumn)> {
        self.longest_measured
    }
}

fn display_width_chunk(column: usize, text: &str, config: &BufferConfig) -> usize {
    text.graphemes(true).fold(column, |column, grapheme| {
        advance_display_column(column, grapheme, config)
    })
}

pub(super) fn line_content(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

pub(crate) fn advance_display_column(
    column: usize,
    grapheme: &str,
    config: &BufferConfig,
) -> usize {
    if grapheme == "\t" {
        let tab_width = config.tab.tab_width();
        return column + tab_width - column % tab_width;
    }
    let Some(first) = grapheme.chars().next() else {
        return column;
    };
    column + char_width(first)
}

/// 在给定文本内把字节边界映射到 display-column。
pub(crate) fn display_column_for_byte(
    text: &str,
    start_column: usize,
    target_byte: usize,
    config: &BufferConfig,
) -> usize {
    let mut display = start_column;
    let mut byte = 0;
    for grapheme in text.graphemes(true) {
        if target_byte <= byte {
            break;
        }
        let next_byte = byte + grapheme.len();
        if target_byte < next_byte {
            break;
        }
        display = advance_display_column(display, grapheme, config);
        byte = next_byte;
    }
    display
}

/// 在给定文本内把 display-column 映射回字节位置。
///
/// 文本首字符所处的显示列（软换行续行从假空格缩进后的列开始，tab 对齐必须基于行内绝对列而非片段内相对列）。
/// 目标列落在某个 grapheme 中间时吸附到最近边界（距离相等取前）；超出文本末尾返回 `text.len()`。
pub(crate) fn byte_for_display_column(
    text: &str,
    start_column: usize,
    target_column: usize,
    config: &BufferConfig,
) -> usize {
    if target_column <= start_column {
        return 0;
    }
    let mut display = start_column;
    let mut byte = 0;
    for grapheme in text.graphemes(true) {
        if target_column == display {
            return byte;
        }
        let next_display = advance_display_column(display, grapheme, config);
        let next_byte = byte + grapheme.len();
        if target_column == next_display {
            return next_byte;
        }
        if target_column > display && target_column < next_display {
            return if target_column - display <= next_display - target_column {
                byte
            } else {
                next_byte
            };
        }
        display = next_display;
        byte = next_byte;
    }
    text.len()
}

#[cfg(test)]
mod test {
    use super::*;

    impl TabMap {
        pub(crate) fn measured_lines(&self) -> impl Iterator<Item = (Line, DisplayColumn)> + '_ {
            self.measured_line_widths
                .iter()
                .map(|(line, width)| (*line, *width))
        }
    }
}
