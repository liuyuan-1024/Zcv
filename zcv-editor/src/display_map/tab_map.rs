//! DisplayMap 的 Tab 展开与 display-column 映射。
//!
//! `TabMap` 只测量实际进入投影视口的逻辑行，并在同行编辑后精确失效对应缓存。
//! 初次构建不遍历全文；结构编辑会清空已测量行，但后续仍按需重新填充。

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;

use unicode_segmentation::UnicodeSegmentation;
use zcv_engine::{
    ByteOffset, DisplayColumn, DisplayColumnAffinity, Line, LogicalColumn, Position, Snapshot,
};

use super::{
    error::DisplayMapResult,
    fold_map::{FoldEdit, FoldSnapshot},
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

    pub(super) fn fold_snapshot(&self) -> &FoldSnapshot {
        &self.fold_snapshot
    }

    pub(super) fn buffer_snapshot(&self) -> &Snapshot {
        self.fold_snapshot.buffer_snapshot()
    }

    #[cfg(test)]
    pub(super) const fn version(&self) -> u64 {
        self.version
    }

    pub(super) fn logical_to_display_column(
        &self,
        line: Line,
        column: LogicalColumn,
    ) -> DisplayMapResult<DisplayColumn> {
        let snapshot = self.buffer_snapshot();
        let end = snapshot.position_to_byte(Position::new(line, column))?;
        let start = snapshot.line_start_byte(line)?;
        let text = snapshot.slice_byte_range(start, end)?;
        Ok(DisplayColumn::new(display_width(text.as_str(), snapshot)))
    }

    pub(super) fn display_to_logical_column(
        &self,
        line: Line,
        column: DisplayColumn,
    ) -> DisplayMapResult<LogicalColumn> {
        let snapshot = self.buffer_snapshot();
        let text = snapshot.slice_line(line)?;
        let text = line_content(text.as_str());
        let target = column.get();
        let affinity = snapshot.config().display_width.affinity;
        let mut display = 0usize;
        let mut logical = 0usize;

        for grapheme in text.graphemes(true) {
            let next_display = advance_display_column(display, grapheme, snapshot);
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

impl Deref for TabSnapshot {
    type Target = FoldSnapshot;

    fn deref(&self) -> &Self::Target {
        &self.fold_snapshot
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

    pub(super) fn snapshot(&self) -> TabSnapshot {
        self.snapshot.clone()
    }

    pub(super) fn sync(
        &mut self,
        fold_snapshot: FoldSnapshot,
        fold_edits: &[FoldEdit],
    ) -> TabSnapshot {
        let snapshot = fold_snapshot.buffer_snapshot();
        let previous_snapshot = self.snapshot.buffer_snapshot();
        let same_configuration = previous_snapshot.config().tab == snapshot.config().tab
            && previous_snapshot.config().display_width == snapshot.config().display_width;
        let same_buffer_version = previous_snapshot.version() == snapshot.version();
        let same_fold_version = self.snapshot.fold_snapshot.version() == fold_snapshot.version();

        if same_configuration && same_fold_version {
            self.snapshot.fold_snapshot = fold_snapshot;
            return self.snapshot.clone();
        }

        let new_version = self.snapshot.version + 1;

        // 折叠状态可以在 Buffer 版本不变时发布新的 FoldSnapshot。此时逻辑行宽
        // 仍然有效，但 TabSnapshot 自己的版本必须前进。
        if same_configuration && same_buffer_version {
            self.snapshot = TabSnapshot {
                fold_snapshot,
                version: new_version,
            };
            return self.snapshot.clone();
        }

        if !same_configuration {
            self.measured_line_widths.clear();
            self.snapshot = TabSnapshot {
                fold_snapshot,
                version: new_version,
            };
            return self.snapshot.clone();
        }

        let structural = fold_edits.iter().any(FoldEdit::is_structural);
        if structural {
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
        let snapshot = self.snapshot.buffer_snapshot();
        let text = snapshot.slice_line(line)?;
        let width = DisplayColumn::new(display_width(line_content(text.as_str()), snapshot));
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
    tab_snapshot: &TabSnapshot,
    line: Line,
    column: DisplayColumn,
) -> DisplayMapResult<ByteOffset> {
    let snapshot = tab_snapshot.buffer_snapshot();
    let logical = tab_snapshot.display_to_logical_column(line, column)?;
    Ok(snapshot.position_to_byte(Position::new(line, logical))?)
}
