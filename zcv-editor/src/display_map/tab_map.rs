//! DisplayMap 的硬 Tab 展开与 Tab 点坐标。
//!
//! Tab 层不保存逐行宽度或变换树。
//! 读取 chunk 时由当前行的 Tab 点即时展开，因而同一行中的后续 Tab 总是以其真实的前置显示列计算。

use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use zcv_multi_buffer::MultiBufferSnapshot;
use zcv_text::{CoordinateError, Line};

use super::chunk::{ChunkText, FoldChunks, HighlightStyles, StyledChunks};
use super::display_width::char_width;
use super::edit::ProjectionEdit;
use super::error::DisplayMapResult;
use super::fold_map::{
    FoldBias, FoldEdit, FoldPoint, FoldSnapshot, ProjectedLineIndex, StreamProjectedKind,
};

/// Tab 层的本层点。行表示投影行，列表示展开硬 Tab 后的显示列。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TabPoint {
    row: usize,
    column: usize,
}

impl TabPoint {
    pub(crate) const fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }

    pub(crate) const fn zero() -> Self {
        Self::new(0, 0)
    }

    pub(crate) const fn row(self) -> usize {
        self.row
    }

    pub(crate) const fn column(self) -> usize {
        self.column
    }

    /// 把一段文本摘要（行增量与尾列）推进到下一个点。
    pub(super) const fn advance(self, summary: Self) -> Self {
        if summary.row == 0 {
            Self::new(self.row, self.column + summary.column)
        } else {
            Self::new(self.row + summary.row, summary.column)
        }
    }
}

/// Tab 层的本层编辑：旧区间属于旧 Tab 快照，新区间属于新 Tab 快照。
pub(super) type TabEdit = ProjectionEdit<TabPoint>;

#[derive(Debug, Clone)]
pub(crate) struct TabSnapshot {
    fold_snapshot: FoldSnapshot,
    version: u64,
    tab_width: NonZeroUsize,
}

impl TabSnapshot {
    pub(super) fn new(fold_snapshot: FoldSnapshot, tab_width: NonZeroUsize) -> Self {
        Self {
            fold_snapshot,
            version: 0,
            tab_width,
        }
    }

    pub(crate) fn tab_width(&self) -> NonZeroUsize {
        self.tab_width
    }

    pub(super) fn buffer_snapshot(&self) -> &MultiBufferSnapshot {
        self.fold_snapshot.buffer_snapshot()
    }

    pub(crate) fn fold_snapshot(&self) -> &FoldSnapshot {
        &self.fold_snapshot
    }

    /// 投影行总数由下层 Fold 快照定义，不由本层编辑的行差推导。
    pub(super) fn line_count(&self) -> usize {
        self.fold_snapshot.line_count()
    }

    pub(super) fn max_point(&self) -> TabPoint {
        self.fold_point_to_tab_point(self.fold_snapshot.max_point())
    }

    /// 一段 Tab 文本范围的相对终点摘要。它可作为 Wrap 变换树的输入维度。
    pub(super) fn summary_for_range(&self, range: Range<TabPoint>) -> TabPoint {
        debug_assert!(range.start <= range.end);
        if range.start.row == range.end.row {
            TabPoint::new(0, range.end.column - range.start.column)
        } else {
            TabPoint::new(range.end.row - range.start.row, range.end.column)
        }
    }

    /// 投影行 → 对应的 buffer 行来源。
    pub(super) fn projected_kind(&self, line: Line) -> Option<StreamProjectedKind> {
        self.fold_snapshot
            .projected_kind(ProjectedLineIndex::new(line.get()))
    }

    /// 投影行 → 行文本（经折叠投影；合并行是按段合成的文本）。
    pub(super) fn line_text(&self, line: Line) -> Option<Cow<'_, str>> {
        let fold = self.fold_snapshot();
        let projected = ProjectedLineIndex::new(line.get());
        if fold.is_fold_row(projected) {
            return fold.row_text(projected);
        }
        let stream_line = self.stream_line_for_projected(line)?;
        fold.buffer_snapshot().line_text(stream_line)
    }

    /// 投影行 → 字节范围（折叠合并行锚定至其首个源行）。
    pub(super) fn line_byte_range(
        &self,
        line: Line,
    ) -> Option<Range<zcv_multi_buffer::MultiBufferOffset>> {
        let fold = self.fold_snapshot();
        let projected = ProjectedLineIndex::new(line.get());
        if let Some(anchor_stream) = fold.fold_row_anchor_stream_line(projected) {
            let start = fold.buffer_snapshot().line_start_byte(anchor_stream).ok()?;
            return Some(start..start);
        }
        let stream_line = self.stream_line_for_projected(line)?;
        fold.buffer_snapshot().line_byte_range(stream_line)
    }

    /// 投影行 → 流行号（坐标换算用）。
    pub(super) fn stream_line_for_projected(&self, line: Line) -> Option<Line> {
        match self.projected_kind(line)? {
            StreamProjectedKind::Text(source) => Some(source),
        }
    }

    pub(super) const fn version(&self) -> u64 {
        self.version
    }

    /// Fold 点转换成 Tab 点。Fold 列是输出字节列，Tab 列是展开后的显示列。
    pub(crate) fn fold_point_to_tab_point(&self, point: FoldPoint) -> TabPoint {
        let line = Line::new(point.row());
        let Some(text) = self.line_text(line) else {
            // 组合投影的尾部端点没有可读取的行内容，但仍是合法的层间区间端点。
            // 它没有可展开的 Tab，保留同值列即可。
            return TabPoint::new(point.row(), point.column());
        };
        let content = line_content(text.as_ref());
        let column = display_column_for_byte(
            content,
            0,
            point.column().min(content.len()),
            self.tab_width.get(),
        );
        let tab_point = TabPoint::new(point.row(), column);
        debug_assert_eq!(
            self.tab_point_to_fold_point(tab_point, FoldBias::Left)
                .row(),
            point.row(),
            "Tab/Fold 点转换不得跨投影行"
        );
        tab_point
    }

    /// Tab 点反向转换成 Fold 点。落在 Tab 展开空格中的点按显式 Bias 吸附。
    pub(crate) fn tab_point_to_fold_point(&self, point: TabPoint, bias: FoldBias) -> FoldPoint {
        let line = Line::new(point.row());
        let Some(text) = self.line_text(line) else {
            return FoldPoint::new(point.row(), point.column());
        };
        let content = line_content(text.as_ref());
        FoldPoint::new(
            point.row(),
            byte_for_display_column_with_bias(
                content,
                0,
                point.column(),
                self.tab_width.get(),
                bias,
            ),
        )
    }
}

#[derive(Debug, Clone)]
pub(super) struct TabMap {
    snapshot: TabSnapshot,
}

impl TabMap {
    pub(super) fn new(fold_snapshot: FoldSnapshot, tab_width: NonZeroUsize) -> (Self, TabSnapshot) {
        let snapshot = TabSnapshot::new(fold_snapshot, tab_width);
        (
            Self {
                snapshot: snapshot.clone(),
            },
            snapshot,
        )
    }

    pub(super) fn snapshot(&self) -> &TabSnapshot {
        &self.snapshot
    }

    pub(super) fn sync(
        &mut self,
        fold_snapshot: FoldSnapshot,
        fold_edits: &[FoldEdit],
        tab_width: NonZeroUsize,
    ) -> (TabSnapshot, Vec<TabEdit>) {
        let old_snapshot = self.snapshot.clone();
        let configuration_changed = old_snapshot.tab_width != tab_width;
        let fold_changed = old_snapshot.fold_snapshot.version() != fold_snapshot.version();
        let mut next = TabSnapshot {
            fold_snapshot,
            version: old_snapshot.version + u64::from(configuration_changed || fold_changed),
            tab_width,
        };

        let edits = if configuration_changed {
            vec![ProjectionEdit::new(
                TabPoint::zero()..old_snapshot.max_point(),
                TabPoint::zero()..next.max_point(),
            )]
        } else {
            tab_edits_from_fold_edits(&old_snapshot, &next, fold_edits)
        };

        if !configuration_changed && !fold_changed {
            next.version = old_snapshot.version;
        }
        self.snapshot = next;
        (self.snapshot.clone(), edits)
    }
}

/// Fold 层的精确字节编辑直接映射为 Tab 层的精确点编辑。
///
/// 不再把端点降为行区间，也不以各编辑行数差校验或补偿整个投影。
fn tab_edits_from_fold_edits(
    old_snapshot: &TabSnapshot,
    new_snapshot: &TabSnapshot,
    fold_edits: &[FoldEdit],
) -> Vec<TabEdit> {
    fold_edits
        .iter()
        .map(|edit| {
            ProjectionEdit::new(
                old_snapshot
                    .fold_point_to_tab_point(edit.old.start.to_point(old_snapshot.fold_snapshot()))
                    ..old_snapshot.fold_point_to_tab_point(
                        edit.old.end.to_point(old_snapshot.fold_snapshot()),
                    ),
                new_snapshot
                    .fold_point_to_tab_point(edit.new.start.to_point(new_snapshot.fold_snapshot()))
                    ..new_snapshot.fold_point_to_tab_point(
                        edit.new.end.to_point(new_snapshot.fold_snapshot()),
                    ),
            )
        })
        .collect()
}

pub(super) fn line_content(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

pub(crate) fn advance_display_column(column: usize, grapheme: &str, tab_width: usize) -> usize {
    if grapheme == "\t" {
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
    tab_width: usize,
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
        display = advance_display_column(display, grapheme, tab_width);
        byte = next_byte;
    }
    display
}

/// 在给定文本内把 display-column 映射回字节位置。
pub(crate) fn byte_for_display_column(
    text: &str,
    start_column: usize,
    target_column: usize,
    tab_width: usize,
) -> usize {
    byte_for_display_column_with_bias(text, start_column, target_column, tab_width, FoldBias::Left)
}

fn byte_for_display_column_with_bias(
    text: &str,
    start_column: usize,
    target_column: usize,
    tab_width: usize,
    bias: FoldBias,
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
        let next_display = advance_display_column(display, grapheme, tab_width);
        let next_byte = byte + grapheme.len();
        if target_column == next_display {
            return next_byte;
        }
        if target_column > display && target_column < next_display {
            return match bias {
                FoldBias::Left => byte,
                FoldBias::Right => next_byte,
            };
        }
        display = next_display;
        byte = next_byte;
    }
    text.len()
}

/// Tab 读取时才展开硬 Tab 的连续 chunk；不存在逐行宽度缓存。
pub(super) fn display_width_for_fold_row(
    snapshot: &TabSnapshot,
    row: Line,
) -> DisplayMapResult<usize> {
    let fold = snapshot.fold_snapshot();
    let projected = ProjectedLineIndex::new(row.get());
    let mut width = 0;
    if let Some(segments) = fold.fold_row_segments(projected) {
        let content_len = segments
            .last()
            .expect("折叠合并行必须至少包含一个段")
            .merged_range()
            .end;
        for chunk in FoldChunks::new(
            &segments,
            fold.buffer_snapshot(),
            HighlightStyles::default(),
            0..content_len,
        ) {
            width = chunk.text.graphemes(true).fold(width, |column, grapheme| {
                advance_display_column(column, grapheme, snapshot.tab_width().get())
            });
        }
    } else {
        let stream_line = snapshot
            .stream_line_for_projected(row)
            .ok_or(CoordinateError::LineOutOfBounds(row))?;
        let buffer = fold.buffer_snapshot();
        let range = buffer
            .line_content_byte_range(stream_line)
            .ok_or(CoordinateError::LineOutOfBounds(row))?;
        let content_len = buffer
            .line_content_metrics(stream_line)
            .ok_or(CoordinateError::LineOutOfBounds(row))?
            .0;
        for chunk in StyledChunks::new(
            ChunkText::Virtual {
                snapshot: buffer,
                range: range.clone(),
            },
            range.start.get(),
            0,
            HighlightStyles::default(),
            0..content_len,
        ) {
            width = chunk.text.graphemes(true).fold(width, |column, grapheme| {
                advance_display_column(column, grapheme, snapshot.tab_width().get())
            });
        }
    }
    Ok(width)
}
