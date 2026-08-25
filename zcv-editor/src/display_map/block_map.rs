//! 多文件 Editor 的块级显示投影。
//!
//! 本层位于 WrapMap 之上：文本换行坐标保持不变，文件标题和同文件片段分隔线作为不属于文本的虚拟显示块插入。
//! 这样搜索、diff、诊断等宿主只负责提供 excerpts，滚动、命中测试、选区和通用文件标题都由 Editor 复用同一条管线。

use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;

use zcv_multi_buffer::ExcerptSnapshot;
use zcv_text::{ByteOffset, CoordinateError, Line, LogicalColumn, TextRange};

use super::error::DisplayMapResult;
use super::fold_map::{ProjectedLineIndex, ProjectedPoint, ProjectedRange};
use super::wrap_map::{WrapSnapshot, WrapViewportRowKind};
use super::{DisplayPoint, DisplayRow};

pub(super) const FILE_HEADER_HEIGHT: usize = 2;
pub(super) const EXCERPT_BOUNDARY_HEIGHT: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayBlockKind {
    BufferHeader,
    ExcerptBoundary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DisplayBlock {
    pub(crate) kind: DisplayBlockKind,
    pub(crate) excerpt: ExcerptSnapshot,
}

#[derive(Clone, Debug)]
struct BlockPlacement {
    display_row: usize,
    height: usize,
    block: DisplayBlock,
    hidden_wrap_range: Option<Range<usize>>,
}

#[derive(Clone, Copy, Debug)]
enum DisplayRowEntry {
    Text(usize),
    Block(usize),
}

#[derive(Debug, Clone)]
pub(super) struct BlockSnapshot {
    wrap_snapshot: WrapSnapshot,
    placements: Vec<BlockPlacement>,
    rows: Vec<DisplayRowEntry>,
    wrap_to_display: Vec<Option<usize>>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DisplayViewportRow<'a> {
    index: DisplayRow,
    height: usize,
    kind: DisplayViewportItemKind<'a>,
}

impl<'a> DisplayViewportRow<'a> {
    pub(crate) fn index(&self) -> DisplayRow {
        self.index
    }

    pub(crate) fn height(&self) -> usize {
        self.height
    }

    pub(crate) fn kind(&self) -> &WrapViewportRowKind<'a> {
        match &self.kind {
            DisplayViewportItemKind::Text(kind) => kind,
            DisplayViewportItemKind::Block(_) => {
                panic!("虚拟块没有文本行类型；调用方应先查询 block()")
            }
        }
    }

    pub(crate) fn block(&self) -> Option<&DisplayBlock> {
        match &self.kind {
            DisplayViewportItemKind::Text(_) => None,
            DisplayViewportItemKind::Block(block) => Some(block),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum DisplayViewportItemKind<'a> {
    Text(WrapViewportRowKind<'a>),
    Block(DisplayBlock),
}

#[derive(Debug, Clone)]
pub(crate) struct DisplayViewportSlice<'a> {
    rows: Vec<DisplayViewportRow<'a>>,
}

impl<'a> DisplayViewportSlice<'a> {
    pub(crate) fn rows(&self) -> &[DisplayViewportRow<'a>] {
        &self.rows
    }
}

enum RowMapping<'a> {
    Text(DisplayRow),
    Block(&'a BlockPlacement),
}

impl BlockSnapshot {
    pub(super) fn new(
        wrap_snapshot: WrapSnapshot,
        excerpts: &[ExcerptSnapshot],
        folded_buffers: &HashSet<PathBuf>,
    ) -> Self {
        struct BlockSpec {
            wrap_row: usize,
            height: usize,
            block: DisplayBlock,
            hide_until: Option<usize>,
        }

        let excerpt_starts = excerpts
            .iter()
            .filter_map(|excerpt| {
                wrap_snapshot
                    .offset_to_display_point(excerpt.output_range().start())
                    .ok()
                    .map(|point| (point.row().get(), excerpt.clone()))
            })
            .collect::<Vec<_>>();
        let mut specs = Vec::new();
        let mut group_start = 0usize;
        while group_start < excerpt_starts.len() {
            let path = excerpt_starts[group_start].1.path();
            let mut group_end = group_start + 1;
            while group_end < excerpt_starts.len() && excerpt_starts[group_end].1.path() == path {
                group_end += 1;
            }
            let wrap_end = excerpt_starts
                .get(group_end)
                .map_or_else(|| wrap_snapshot.line_count(), |(row, _)| *row);
            if folded_buffers.contains(path) {
                let (wrap_row, excerpt) = &excerpt_starts[group_start];
                specs.push(BlockSpec {
                    wrap_row: *wrap_row,
                    height: FILE_HEADER_HEIGHT,
                    block: DisplayBlock {
                        kind: DisplayBlockKind::BufferHeader,
                        excerpt: excerpt.clone(),
                    },
                    hide_until: Some(wrap_end),
                });
            } else {
                for (index, (wrap_row, excerpt)) in
                    excerpt_starts[group_start..group_end].iter().enumerate()
                {
                    let kind = if index == 0 {
                        DisplayBlockKind::BufferHeader
                    } else {
                        DisplayBlockKind::ExcerptBoundary
                    };
                    specs.push(BlockSpec {
                        wrap_row: *wrap_row,
                        height: match kind {
                            DisplayBlockKind::BufferHeader => FILE_HEADER_HEIGHT,
                            DisplayBlockKind::ExcerptBoundary => EXCERPT_BOUNDARY_HEIGHT,
                        },
                        block: DisplayBlock {
                            kind,
                            excerpt: excerpt.clone(),
                        },
                        hide_until: None,
                    });
                }
            }
            group_start = group_end;
        }

        let wrap_line_count = wrap_snapshot.line_count();
        let mut placements = Vec::new();
        let mut rows = Vec::new();
        let mut wrap_to_display = vec![None; wrap_line_count];
        let mut wrap_row = 0usize;
        for spec in specs {
            while wrap_row < spec.wrap_row.min(wrap_line_count) {
                wrap_to_display[wrap_row] = Some(rows.len());
                rows.push(DisplayRowEntry::Text(wrap_row));
                wrap_row += 1;
            }
            let placement_index = placements.len();
            let display_row = rows.len();
            placements.push(BlockPlacement {
                display_row,
                height: spec.height,
                block: spec.block,
                hidden_wrap_range: spec
                    .hide_until
                    .map(|end| spec.wrap_row..end.min(wrap_line_count)),
            });
            rows.extend((0..spec.height).map(|_| DisplayRowEntry::Block(placement_index)));
            if let Some(end) = spec.hide_until {
                wrap_row = wrap_row.max(end.min(wrap_line_count));
            }
        }
        while wrap_row < wrap_line_count {
            wrap_to_display[wrap_row] = Some(rows.len());
            rows.push(DisplayRowEntry::Text(wrap_row));
            wrap_row += 1;
        }

        Self {
            wrap_snapshot,
            placements,
            rows,
            wrap_to_display,
        }
    }

    pub(super) fn line_count(&self) -> usize {
        self.rows.len()
    }

    fn wrap_row_to_display_row(&self, wrap_row: usize) -> usize {
        if let Some(Some(display_row)) = self.wrap_to_display.get(wrap_row) {
            return *display_row;
        }
        self.placements
            .iter()
            .find(|placement| {
                placement
                    .hidden_wrap_range
                    .as_ref()
                    .is_some_and(|range| range.contains(&wrap_row))
            })
            .map_or_else(
                || self.rows.len().saturating_sub(1),
                |placement| placement.display_row,
            )
    }

    pub(super) fn display_row_to_wrap_row(&self, display_row: DisplayRow) -> Option<DisplayRow> {
        if display_row.get() >= self.rows.len() {
            return None;
        }
        match self.display_row_mapping(display_row.get()) {
            RowMapping::Text(row) => Some(row),
            RowMapping::Block(_) => None,
        }
    }

    pub(super) fn projected_wrap_row_to_display_row(&self, wrap_row: usize) -> DisplayRow {
        DisplayRow::new(self.wrap_row_to_display_row(wrap_row))
    }

    fn display_row_mapping(&self, display_row: usize) -> RowMapping<'_> {
        match self.rows.get(display_row).or_else(|| self.rows.last()) {
            Some(DisplayRowEntry::Text(wrap_row)) => RowMapping::Text(DisplayRow::new(*wrap_row)),
            Some(DisplayRowEntry::Block(placement)) => {
                RowMapping::Block(&self.placements[*placement])
            }
            None => RowMapping::Text(DisplayRow::ZERO),
        }
    }

    pub(super) fn offset_to_display_point(
        &self,
        offset: ByteOffset,
    ) -> DisplayMapResult<DisplayPoint> {
        let point = self.wrap_snapshot.offset_to_display_point(offset)?;
        Ok(DisplayPoint::new(
            DisplayRow::new(self.wrap_row_to_display_row(point.row().get())),
            point.column(),
        ))
    }

    pub(super) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<ByteOffset> {
        if point.row().get() >= self.rows.len() {
            return Err(CoordinateError::LineOutOfBounds(Line::new(point.row().get())).into());
        }
        match self.display_row_mapping(point.row().get()) {
            RowMapping::Text(row) => self
                .wrap_snapshot
                .display_point_to_offset(DisplayPoint::new(row, point.column())),
            RowMapping::Block(placement) => Ok(placement.block.excerpt.output_range().start()),
        }
    }

    pub(super) fn project_text_range(
        &self,
        range: TextRange,
    ) -> DisplayMapResult<Vec<ProjectedRange>> {
        self.wrap_snapshot
            .project_text_range(range)?
            .into_iter()
            .map(|range| {
                let start = range.start();
                let end = range.end();
                ProjectedRange::new(
                    ProjectedPoint::new(
                        ProjectedLineIndex::new(self.wrap_row_to_display_row(start.line().get())),
                        start.column(),
                    ),
                    ProjectedPoint::new(
                        ProjectedLineIndex::new(self.wrap_row_to_display_row(end.line().get())),
                        end.column(),
                    ),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    pub(super) fn slice_viewport(
        &self,
        start_row: DisplayRow,
        line_count: usize,
    ) -> DisplayMapResult<DisplayViewportSlice<'_>> {
        let end = start_row
            .get()
            .saturating_add(line_count)
            .min(self.line_count());
        let mut row = start_row.get().min(self.line_count().saturating_sub(1));
        let mut rows = Vec::new();

        while row < end {
            match self.display_row_mapping(row) {
                RowMapping::Block(placement) => {
                    rows.push(DisplayViewportRow {
                        index: DisplayRow::new(placement.display_row),
                        height: placement.height,
                        kind: DisplayViewportItemKind::Block(placement.block.clone()),
                    });
                    row = placement.display_row + placement.height;
                }
                RowMapping::Text(wrap_row) => {
                    let viewport = self.wrap_snapshot.slice_viewport(wrap_row, 1)?;
                    let Some(wrap) = viewport.rows().first() else {
                        break;
                    };
                    rows.push(DisplayViewportRow {
                        index: DisplayRow::new(row),
                        height: 1,
                        kind: DisplayViewportItemKind::Text(wrap.kind().clone()),
                    });
                    row += 1;
                }
            }
        }

        Ok(DisplayViewportSlice { rows })
    }

    pub(super) fn line_to_display_row(&self, offset: ByteOffset) -> Option<DisplayRow> {
        self.offset_to_display_point(offset)
            .ok()
            .map(DisplayPoint::row)
    }

    pub(super) fn display_to_logical_column(
        &self,
        line: zcv_text::Line,
        column: super::DisplayColumn,
    ) -> DisplayMapResult<LogicalColumn> {
        self.wrap_snapshot
            .tab_snapshot()
            .display_to_logical_column(line, column)
    }
}
