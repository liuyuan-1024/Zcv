//! DisplayMap 的 Tab 展开与 display-column 映射。
//!
//! `TabMap` 只测量实际进入投影视口的逻辑行，并在同行编辑后精确失效对应缓存。
//! 初次构建不遍历全文；结构编辑会清空已测量行，但后续仍按需重新填充。

use std::collections::{BTreeMap, BTreeSet};

use unicode_segmentation::UnicodeSegmentation;
use zcv_engine::{
    ByteOffset, DeltaEvent, DisplayColumn, DisplayColumnAffinity, Line, LogicalColumn, Position,
    Snapshot,
};

use super::error::DisplayMapResult;

#[derive(Debug, Clone)]
pub(super) struct TabSnapshot {
    snapshot: Snapshot,
}

impl TabSnapshot {
    pub(super) fn new(snapshot: Snapshot) -> Self {
        Self { snapshot }
    }

    pub(super) fn logical_to_display_column(
        &self,
        line: Line,
        column: LogicalColumn,
    ) -> DisplayMapResult<DisplayColumn> {
        let end = self
            .snapshot
            .position_to_byte(Position::new(line, column))?;
        let start = self.snapshot.line_start_byte(line)?;
        let text = self.snapshot.slice_byte_range(start, end)?;
        Ok(DisplayColumn::new(display_width(
            text.as_str(),
            &self.snapshot,
        )))
    }

    pub(super) fn display_to_logical_column(
        &self,
        line: Line,
        column: DisplayColumn,
    ) -> DisplayMapResult<LogicalColumn> {
        let text = self.snapshot.slice_line(line)?;
        let text = line_content(text.as_str());
        let target = column.get();
        let affinity = self.snapshot.config().display_width.affinity;
        let mut display = 0usize;
        let mut logical = 0usize;

        for grapheme in text.graphemes(true) {
            let next_display = advance_display_column(display, grapheme, &self.snapshot);
            let next_logical = logical + grapheme.chars().count();

            if target == display {
                return Ok(LogicalColumn::new(logical));
            }
            if target == next_display {
                return Ok(LogicalColumn::new(next_logical));
            }
            if target > display && target < next_display {
                return Ok(LogicalColumn::new(match affinity {
                    DisplayColumnAffinity::Previous => logical,
                    DisplayColumnAffinity::Next => next_logical,
                    DisplayColumnAffinity::Nearest => {
                        if target - display <= next_display - target {
                            logical
                        } else {
                            next_logical
                        }
                    }
                }));
            }

            display = next_display;
            logical = next_logical;
        }

        Ok(LogicalColumn::new(logical))
    }
}

#[derive(Debug, Clone)]
pub(super) struct TabMap {
    snapshot: TabSnapshot,
    measured_line_widths: BTreeMap<Line, DisplayColumn>,
}

impl TabMap {
    pub(super) fn new(snapshot: Snapshot) -> Self {
        Self {
            snapshot: TabSnapshot::new(snapshot),
            measured_line_widths: BTreeMap::new(),
        }
    }

    pub(super) fn snapshot(&self) -> TabSnapshot {
        self.snapshot.clone()
    }

    pub(super) fn sync(&mut self, snapshot: Snapshot, event: Option<&DeltaEvent>) {
        let same_configuration = self.snapshot.snapshot.config().tab == snapshot.config().tab
            && self.snapshot.snapshot.config().display_width == snapshot.config().display_width;
        let compatible = event.is_some_and(|event| {
            event.old_version() == self.snapshot.snapshot.version()
                && event.new_version() == snapshot.version()
        });

        if !same_configuration || !compatible {
            self.measured_line_widths.clear();
            self.snapshot = TabSnapshot::new(snapshot);
            return;
        }

        let event = event.expect("compatible 已确认 DeltaEvent 存在");
        let structural = event.delta().edits().as_slice().iter().any(|edit| {
            edit.replacement().contains('\n')
                || self
                    .snapshot
                    .snapshot
                    .slice_text(edit.range())
                    .is_ok_and(|text| text.as_str().contains('\n'))
        });
        if structural {
            self.measured_line_widths.clear();
        } else if let Ok(ranges) = event.changeset().changed_ranges() {
            let mut changed_lines = BTreeSet::new();
            for range in ranges {
                if let Ok(start) = snapshot.byte_to_line(range.start())
                    && let Ok(end) = snapshot.byte_to_line(range.end())
                {
                    changed_lines.extend((start.get()..=end.get()).map(Line::new));
                }
            }
            self.measured_line_widths
                .retain(|line, _| !changed_lines.contains(line));
        } else {
            self.measured_line_widths.clear();
        }
        self.snapshot = TabSnapshot::new(snapshot);
    }

    pub(super) fn measure_line(&mut self, line: Line) -> DisplayMapResult<DisplayColumn> {
        if let Some(width) = self.measured_line_widths.get(&line) {
            return Ok(*width);
        }
        let text = self.snapshot.snapshot.slice_line(line)?;
        let width = DisplayColumn::new(display_width(
            line_content(text.as_str()),
            &self.snapshot.snapshot,
        ));
        self.measured_line_widths.insert(line, width);
        Ok(width)
    }

    pub(super) fn measured_lines(&self) -> impl Iterator<Item = (Line, DisplayColumn)> + '_ {
        self.measured_line_widths
            .iter()
            .map(|(line, width)| (*line, *width))
    }
}

fn line_content(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

fn display_width(text: &str, snapshot: &Snapshot) -> usize {
    text.graphemes(true).fold(0, |column, grapheme| {
        advance_display_column(column, grapheme, snapshot)
    })
}

fn advance_display_column(column: usize, grapheme: &str, snapshot: &Snapshot) -> usize {
    if grapheme == "\t" {
        let tab_width = snapshot.config().tab.tab_width();
        return column + tab_width - column % tab_width;
    }
    let Some(first) = grapheme.chars().next() else {
        return column;
    };
    column + snapshot.config().display_width.char_width(first)
}

pub(super) fn display_column_to_byte(
    snapshot: &Snapshot,
    tab_snapshot: &TabSnapshot,
    line: Line,
    column: DisplayColumn,
) -> DisplayMapResult<ByteOffset> {
    let logical = tab_snapshot.display_to_logical_column(line, column)?;
    Ok(snapshot.position_to_byte(Position::new(line, logical))?)
}
