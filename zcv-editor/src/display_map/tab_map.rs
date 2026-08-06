//! DisplayMap 的 Tab 展开与 display-column 映射。
//!
//! `TabMap` 只测量实际进入投影视口的逻辑行，并在同行编辑后精确失效对应缓存。
//! 初次构建不遍历全文；结构编辑会清空已测量行，但后续仍按需重新填充。

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use super::display_width::{DisplayColumn, char_width};
use unicode_segmentation::UnicodeSegmentation;
use zcv_engine::{ByteOffset, CoordinateError, Line, LogicalColumn, Snapshot};

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

    pub(super) fn buffer_snapshot(&self) -> &Snapshot {
        self.fold_snapshot.buffer_snapshot()
    }

    pub(crate) fn fold_snapshot(&self) -> &FoldSnapshot {
        &self.fold_snapshot
    }

    /// 投影行总数（fold 输出）。
    pub(super) fn line_count(&self) -> usize {
        self.fold_snapshot.line_count()
    }

    /// 投影行 → 内容来源（经流行解析：buffer 行 / 占位符 / 合成行）。
    pub(super) fn projected_kind(&self, line: Line) -> Option<StreamProjectedKind> {
        self.fold_snapshot
            .projected_kind(ProjectedLineIndex::new(line.get()))
    }

    /// 投影行 → 行文本（经流行解析与行内提示注入）。
    pub(super) fn line_text(&self, line: Line) -> Option<Cow<'_, str>> {
        let inlay = self.fold_snapshot.inlay_snapshot();
        let stream_line = self.stream_line_for_projected(line)?;
        inlay.line_text(stream_line)
    }

    /// 投影行 → 字节范围（合成行为锚定行行首的伪坐标）。
    pub(super) fn line_byte_range(&self, line: Line) -> Option<Range<ByteOffset>> {
        let inlay = self.fold_snapshot.inlay_snapshot();
        let stream_line = self.stream_line_for_projected(line)?;
        inlay.line_byte_range(stream_line)
    }

    /// 投影行 → 流行号（坐标换算用；占位符无流行号）。
    pub(super) fn stream_line_for_projected(&self, line: Line) -> Option<Line> {
        let inlay = self.fold_snapshot.inlay_snapshot();
        match self.projected_kind(line)? {
            StreamProjectedKind::Text(source) => match source {
                super::line_stream::StreamLineSource::Buffer(buffer_line) => {
                    Some(inlay.stream().buffer_to_stream(Line::new(buffer_line)))
                }
                super::line_stream::StreamLineSource::Inserted { anchor, index } => {
                    let start = inlay.stream().inserted_block_start(anchor)?;
                    Some(Line::new(start.get() + index))
                }
            },
            StreamProjectedKind::Placeholder(_) => None,
        }
    }

    pub(super) const fn version(&self) -> u64 {
        self.version
    }

    pub(super) fn display_to_logical_column(
        &self,
        line: Line,
        column: DisplayColumn,
    ) -> DisplayMapResult<LogicalColumn> {
        // line 是投影行；合成行无逻辑列语义（命中测试映射到锚定行，正常不会到达这里）。
        let Some(text) = self.line_text(line) else {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        };
        let snapshot = self.stream().buffer_snapshot();
        let text = line_content(text.as_ref());
        let target = column.get();
        // 吸附固定为最近边界（目标列落在多列字符中间时取更近的一端；距离相等取前）。
        let mut display = 0usize;
        let mut projected_byte = 0usize;

        for grapheme in text.graphemes(true) {
            let next_display = advance_display_column(display, grapheme, snapshot);
            let next_byte = projected_byte + grapheme.len();

            if target == display {
                return self.logical_column_at(line, projected_byte);
            }
            if target == next_display {
                return self.logical_column_at(line, next_byte);
            }
            if target > display && target < next_display {
                return self.logical_column_at(
                    line,
                    if target - display <= next_display - target {
                        projected_byte
                    } else {
                        next_byte
                    },
                );
            }

            display = next_display;
            projected_byte = next_byte;
        }

        self.logical_column_at(line, projected_byte)
    }

    /// 投影行内字节 → 逻辑列（原始文本前缀字符数；注入段内吸附到锚定后）。
    fn logical_column_at(
        &self,
        line: Line,
        projected_byte: usize,
    ) -> DisplayMapResult<LogicalColumn> {
        let inlay = self.fold_snapshot.inlay_snapshot();
        let Some(stream_line) = self.stream_line_for_projected(line) else {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        };
        let original_byte = inlay.to_original_offset(stream_line, projected_byte);
        let Some(text) = inlay.stream().line_text(stream_line) else {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        };
        Ok(LogicalColumn::new(
            text.as_str()[..original_byte.min(text.as_str().len())]
                .chars()
                .count(),
        ))
    }
}

#[derive(Debug, Clone)]
pub(super) struct TabMap {
    snapshot: TabSnapshot,
    measured_line_widths: BTreeMap<Line, DisplayColumn>,
}

impl TabMap {
    pub(super) fn new(fold_snapshot: FoldSnapshot) -> (Self, TabSnapshot) {
        let snapshot = TabSnapshot::new(fold_snapshot);
        (
            Self {
                snapshot: snapshot.clone(),
                measured_line_widths: BTreeMap::new(),
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
        // fold 拓扑（折叠/合成行/行内提示变化都会使 fold 版本前进）。
        let same_fold_version = self.snapshot.fold_snapshot.version() == fold_snapshot.version();

        if same_configuration && same_fold_version {
            self.snapshot = TabSnapshot {
                fold_snapshot,
                version: self.snapshot.version,
            };
            return self.snapshot.clone();
        }

        let new_version = self.snapshot.version + 1;
        // 折叠/合成行/行内提示等结构变化会位移投影行号，宽度缓存键随之错位，必须清空；
        // 行内编辑按 changed_lines 精确失效。
        let structural = fold_edits.iter().any(FoldEdit::is_structural);
        if !same_configuration || structural {
            self.measured_line_widths.clear();
        } else {
            let mut changed_lines = BTreeSet::new();
            for edit in fold_edits {
                changed_lines.extend(edit.changed_lines().iter().copied());
            }
            self.measured_line_widths
                .retain(|line, _| !changed_lines.contains(line));
        }
        self.snapshot = TabSnapshot {
            fold_snapshot,
            version: new_version,
        };
        self.snapshot.clone()
    }

    pub(super) fn measure_line(&mut self, line: Line) -> DisplayMapResult<DisplayColumn> {
        if let Some(width) = self.measured_line_widths.get(&line) {
            return Ok(*width);
        }
        // line 是投影行；合成行同样按文本测量（tab 展开/宽度数学不变）。
        let Some(text) = self.snapshot.line_text(line) else {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        };
        let snapshot = self.snapshot.stream().buffer_snapshot();
        let width = DisplayColumn::new(display_width(line_content(text.as_ref()), snapshot));
        self.measured_line_widths.insert(line, width);
        Ok(width)
    }

    pub(super) fn measured_lines(&self) -> impl Iterator<Item = (Line, DisplayColumn)> + '_ {
        self.measured_line_widths
            .iter()
            .map(|(line, width)| (*line, *width))
    }
}

pub(super) fn line_content(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

pub(super) fn display_width(text: &str, snapshot: &Snapshot) -> usize {
    text.graphemes(true).fold(0, |column, grapheme| {
        advance_display_column(column, grapheme, snapshot)
    })
}

pub(super) fn advance_display_column(column: usize, grapheme: &str, snapshot: &Snapshot) -> usize {
    if grapheme == "\t" {
        let tab_width = snapshot.config().tab.tab_width();
        return column + tab_width - column % tab_width;
    }
    let Some(first) = grapheme.chars().next() else {
        return column;
    };
    column + char_width(first)
}

/// 在给定文本内把 display-column 映射回字节位置。
///
/// `start_column` 是文本首字符所处的显示列（软换行续行从假空格缩进后的列开始，tab 对齐必须基于行内绝对列而非片段内相对列）。
/// 目标列落在某个 grapheme 中间时吸附到最近边界（距离相等取前）；超出文本末尾返回 `text.len()`。
pub fn byte_for_display_column(
    text: &str,
    start_column: usize,
    target_column: usize,
    snapshot: &Snapshot,
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
        let next_display = advance_display_column(display, grapheme, snapshot);
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
