//! 折叠显示层。
//!
//! FoldMap 是唯一写入口，向上层发布 FoldSnapshot 与 FoldEdit。

use std::{cmp::Reverse, collections::BTreeMap, ops::Range};

use gpui_sum_tree::{Bias as TreeBias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
#[cfg(test)]
use zcv_engine::{BufferVersion, ByteOffset, Stickiness};
use zcv_engine::{
    CoordinateError, Line, LineRange, LogicalColumn, Position, PositionMap, Snapshot,
    TextChangeBatch, TextRange, TrackedRange, TrackedRangeUpdatePolicy,
};

use super::error::DisplayMapResult;
#[cfg(test)]
use super::error::FoldError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyOutcome {
    Compatible,
    Spliced,
    Rebuilt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct FoldId(u64);

impl FoldId {
    #[cfg(test)]
    const INITIAL: Self = Self(1);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fold {
    id: FoldId,
    range: TrackedRange,
    update_policy: TrackedRangeUpdatePolicy,
    line_span: (Line, Line),
}

impl Fold {
    #[cfg(test)]
    fn new(
        id: FoldId,
        version: BufferVersion,
        range: TextRange,
        update_policy: TrackedRangeUpdatePolicy,
        line_span: (Line, Line),
    ) -> Self {
        Self {
            id,
            range: TrackedRange::from_range(version, range, Stickiness::Never),
            update_policy,
            line_span,
        }
    }

    fn text_range(self) -> TextRange {
        self.range.range()
    }
}

impl Item for Fold {
    type Summary = FoldSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        FoldSummary {
            count: 1,
            last_order: FoldOrder::for_fold(*self),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FoldSummary {
    count: usize,
    last_order: FoldOrder,
}

impl Default for FoldSummary {
    fn default() -> Self {
        Self {
            count: 0,
            last_order: FoldOrder::zero(()),
        }
    }
}

impl ContextLessSummary for FoldSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.count += summary.count;
        self.last_order = summary.last_order;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FoldOrder {
    start: usize,
    end_descending: Reverse<usize>,
    id: FoldId,
}

impl FoldOrder {
    fn for_fold(fold: Fold) -> Self {
        Self {
            start: fold.text_range().start().get(),
            end_descending: Reverse(fold.text_range().end().get()),
            id: fold.id,
        }
    }
}

impl<'a> Dimension<'a, FoldSummary> for FoldOrder {
    fn zero((): ()) -> Self {
        Self {
            start: 0,
            end_descending: Reverse(usize::MAX),
            id: FoldId(0),
        }
    }

    fn add_summary(&mut self, summary: &'a FoldSummary, (): ()) {
        *self = summary.last_order;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransformKind {
    Isomorphic,
    Fold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Transform {
    kind: TransformKind,
    input_lines: usize,
}

impl Transform {
    fn isomorphic(lines: usize) -> Self {
        Self {
            kind: TransformKind::Isomorphic,
            input_lines: lines,
        }
    }

    fn fold(lines: usize) -> Self {
        Self {
            kind: TransformKind::Fold,
            input_lines: lines,
        }
    }

    fn output_rows(self) -> usize {
        match self.kind {
            TransformKind::Isomorphic => self.input_lines,
            TransformKind::Fold => 1,
        }
    }

    fn projected_kind(self, logical_start: usize, offset: usize) -> ProjectedLineKind {
        match self.kind {
            TransformKind::Isomorphic => {
                ProjectedLineKind::Text(TextLine::new(Line::new(logical_start + offset)))
            }
            TransformKind::Fold => {
                debug_assert_eq!(offset, 0);
                let hidden_start = Line::new(logical_start);
                let hidden_end = Line::new(logical_start + self.input_lines);
                ProjectedLineKind::Placeholder(FoldPlaceholder::new(
                    Line::new(logical_start - 1),
                    LineRange::new(hidden_start, hidden_end)
                        .expect("fold transform 必须覆盖非空隐藏行"),
                ))
            }
        }
    }
}

impl Item for Transform {
    type Summary = TransformSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        TransformSummary {
            input_lines: self.input_lines,
            output_rows: self.output_rows(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TransformSummary {
    input_lines: usize,
    output_rows: usize,
}

impl ContextLessSummary for TransformSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.input_lines += summary.input_lines;
        self.output_rows += summary.output_rows;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct InputLines(usize);

impl<'a> Dimension<'a, TransformSummary> for InputLines {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.0 += summary.input_lines;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct OutputRows(usize);

impl<'a> Dimension<'a, TransformSummary> for OutputRows {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.0 += summary.output_rows;
    }
}

type InputToOutput = Dimensions<InputLines, OutputRows>;
type OutputToInput = Dimensions<OutputRows, InputLines>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FoldEdit {
    old: Range<ProjectedLineIndex>,
    new: Range<ProjectedLineIndex>,
    changed_lines: Vec<Line>,
    structural: bool,
}

impl FoldEdit {
    pub(super) fn changed_lines(&self) -> &[Line] {
        &self.changed_lines
    }

    pub(super) fn is_structural(&self) -> bool {
        self.structural || self.old.start != self.new.start || self.old.end != self.new.end
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FoldSnapshot {
    buffer_snapshot: Snapshot,
    folds: SumTree<Fold>,
    transforms: SumTree<Transform>,
    fold_metadata_by_id: BTreeMap<FoldId, TextRange>,
    version: u64,
}

impl FoldSnapshot {
    pub(super) fn buffer_snapshot(&self) -> &Snapshot {
        &self.buffer_snapshot
    }

    pub(super) const fn version(&self) -> u64 {
        self.version
    }

    pub(super) fn line_count(&self) -> usize {
        self.transforms.summary().output_rows
    }

    fn logical_line_count(&self) -> usize {
        self.transforms.summary().input_lines
    }

    pub(super) fn projected_line(&self, index: ProjectedLineIndex) -> Option<ProjectedLine> {
        self.projected_line_kind(index)
            .map(|kind| ProjectedLine::new(index, kind))
    }

    fn projected_line_kind(&self, index: ProjectedLineIndex) -> Option<ProjectedLineKind> {
        let (start, _, transform) =
            self.transforms
                .find::<OutputToInput, _>((), &OutputRows(index.get()), TreeBias::Right);
        transform.map(|transform| {
            transform.projected_kind(start.1.0, index.get().saturating_sub(start.0.0))
        })
    }

    pub(super) fn logical_to_projected(&self, line: Line) -> DisplayMapResult<LogicalProjection> {
        if line.get() >= self.logical_line_count() {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }
        let (start, _, transform) =
            self.transforms
                .find::<InputToOutput, _>((), &InputLines(line.get()), TreeBias::Right);
        let transform = transform.expect("逻辑行必须落在 fold transform 内");
        Ok(match transform.kind {
            TransformKind::Isomorphic => LogicalProjection::Visible(ProjectedLineIndex::new(
                start.1.0 + line.get() - start.0.0,
            )),
            TransformKind::Fold => LogicalProjection::Hidden {
                anchor_logical_line: Line::new(start.0.0 - 1),
                anchor_projected_line: ProjectedLineIndex::new(start.1.0 - 1),
            },
        })
    }

    pub(super) fn logical_to_projected_point(
        &self,
        point: LogicalPoint,
    ) -> DisplayMapResult<LogicalPointProjection> {
        match self.logical_to_projected(point.line())? {
            LogicalProjection::Visible(line) => Ok(LogicalPointProjection::Visible(
                ProjectedPoint::new(line, point.column()),
            )),
            LogicalProjection::Hidden {
                anchor_logical_line,
                anchor_projected_line,
            } => Ok(LogicalPointProjection::Hidden {
                anchor_logical: LogicalPoint::line_start(anchor_logical_line),
                anchor_projected: ProjectedPoint::line_start(anchor_projected_line),
            }),
        }
    }

    pub(super) fn projected_to_logical_point(
        &self,
        point: ProjectedPoint,
    ) -> DisplayMapResult<ProjectedPointMapping> {
        match self
            .projected_line_kind(point.line())
            .ok_or_else(|| CoordinateError::LineOutOfBounds(Line::new(point.line().get())))?
        {
            ProjectedLineKind::Text(text) => Ok(ProjectedPointMapping::Text(LogicalPoint::new(
                text.logical_line(),
                point.column(),
            ))),
            ProjectedLineKind::Placeholder(placeholder) => Ok(ProjectedPointMapping::Placeholder {
                anchor: LogicalPoint::line_start(placeholder.anchor_line()),
                hidden_lines: placeholder.hidden_lines(),
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct FoldMap {
    snapshot: FoldSnapshot,
    #[cfg(test)]
    next_fold_id: FoldId,
    #[cfg(test)]
    default_update_policy: TrackedRangeUpdatePolicy,
}

impl FoldMap {
    pub(super) fn new(buffer_snapshot: Snapshot) -> (Self, FoldSnapshot) {
        let transforms = build_transforms(&[], buffer_snapshot.line_count());
        let snapshot = FoldSnapshot {
            buffer_snapshot,
            folds: SumTree::new(()),
            transforms,
            fold_metadata_by_id: BTreeMap::new(),
            version: 0,
        };
        (
            Self {
                snapshot: snapshot.clone(),
                #[cfg(test)]
                next_fold_id: FoldId::INITIAL,
                #[cfg(test)]
                default_update_policy: TrackedRangeUpdatePolicy::invalidate_when_fully_deleted(),
            },
            snapshot,
        )
    }

    pub(super) fn snapshot(&self) -> &FoldSnapshot {
        &self.snapshot
    }

    pub(super) fn read(
        &mut self,
        current_snapshot: Snapshot,
        batch: &TextChangeBatch,
    ) -> (FoldSnapshot, Vec<FoldEdit>, ApplyOutcome) {
        if current_snapshot.version() == self.snapshot.buffer_snapshot.version() {
            if current_snapshot.config() != self.snapshot.buffer_snapshot.config() {
                let old_end = ProjectedLineIndex::new(self.snapshot.line_count());
                self.snapshot.buffer_snapshot = current_snapshot;
                self.snapshot.version += 1;
                let edit = FoldEdit {
                    old: ProjectedLineIndex::ZERO..old_end,
                    new: ProjectedLineIndex::ZERO
                        ..ProjectedLineIndex::new(self.snapshot.line_count()),
                    changed_lines: Vec::new(),
                    structural: true,
                };
                return (self.snapshot.clone(), vec![edit], ApplyOutcome::Compatible);
            }
            return (self.snapshot.clone(), Vec::new(), ApplyOutcome::Compatible);
        }

        let old_version = self.snapshot.buffer_snapshot.version();
        let new_version = current_snapshot.version();
        if batch.requires_reset()
            || batch.old_version() != Some(old_version)
            || batch.new_version() != Some(new_version)
        {
            let old_rows = self.snapshot.line_count();
            self.snapshot = FoldSnapshot {
                transforms: build_transforms(&[], current_snapshot.line_count()),
                buffer_snapshot: current_snapshot,
                folds: SumTree::new(()),
                fold_metadata_by_id: BTreeMap::new(),
                version: self.snapshot.version + 1,
            };
            let edit = full_fold_edit(old_rows, self.snapshot.line_count());
            return (self.snapshot.clone(), vec![edit], ApplyOutcome::Rebuilt);
        }

        let old_rows = self.snapshot.line_count();
        let old_spans = hidden_spans(&self.snapshot.folds);
        let position_map = PositionMap::from_text_patch(batch.patch());
        let mut retained = Vec::new();
        self.snapshot.fold_metadata_by_id.clear();
        for mut fold in self.snapshot.folds.iter().copied() {
            let update = fold.range.map_through_position_map_with_policy(
                new_version,
                &position_map,
                fold.update_policy,
            );
            if let Some(range) = update.tracked_range() {
                fold.range = range;
                fold.line_span = fold_line_span(&current_snapshot, range.range())
                    .expect("映射后的 tracked fold 必须位于当前 Snapshot 内");
                self.snapshot
                    .fold_metadata_by_id
                    .insert(fold.id, range.range());
                retained.push(fold);
            }
        }
        sort_folds(&mut retained);
        self.snapshot.folds = SumTree::from_iter(retained, ());
        let new_spans = hidden_spans(&self.snapshot.folds);
        let structural = old_spans != new_spans
            || self.snapshot.buffer_snapshot.line_count() != current_snapshot.line_count()
            || batch.patch().edits().iter().any(|edit| {
                self.snapshot
                    .buffer_snapshot
                    .slice_text(edit.old_range())
                    .is_ok_and(|text| text.as_str().contains('\n'))
                    || current_snapshot
                        .slice_text(edit.new_range())
                        .is_ok_and(|text| text.as_str().contains('\n'))
            });
        self.snapshot.buffer_snapshot = current_snapshot;
        self.snapshot.version += 1;
        let outcome = if structural {
            self.snapshot.transforms =
                build_transforms(&new_spans, self.snapshot.buffer_snapshot.line_count());
            ApplyOutcome::Spliced
        } else {
            ApplyOutcome::Compatible
        };
        let edits = if structural {
            vec![full_fold_edit(old_rows, self.snapshot.line_count())]
        } else {
            inline_fold_edits(batch, &self.snapshot.buffer_snapshot)
        };
        (self.snapshot.clone(), edits, outcome)
    }

    #[cfg(test)]
    pub(super) fn write(&mut self) -> FoldMapWriter<'_> {
        FoldMapWriter(self)
    }
}

#[cfg(test)]
pub(super) struct FoldMapWriter<'a>(&'a mut FoldMap);

#[cfg(test)]
impl FoldMapWriter<'_> {
    pub(super) fn fold_lines(
        &mut self,
        line_range: LineRange,
    ) -> DisplayMapResult<(FoldSnapshot, Vec<FoldEdit>)> {
        let range = text_range_for_lines(&self.0.snapshot.buffer_snapshot, line_range)?;
        self.fold(range)
    }

    fn fold(&mut self, range: TextRange) -> DisplayMapResult<(FoldSnapshot, Vec<FoldEdit>)> {
        if range.is_empty() {
            return Err(FoldError::EmptyRange { range }.into());
        }
        if self
            .0
            .snapshot
            .folds
            .iter()
            .any(|fold| fold.text_range() == range)
        {
            return Ok((self.0.snapshot.clone(), Vec::new()));
        }
        for fold in self.0.snapshot.folds.iter() {
            if !ranges_disjoint_or_nested(fold.text_range(), range) {
                return Err(FoldError::OverlapWithoutNesting {
                    existing: fold.text_range(),
                    candidate: range,
                }
                .into());
            }
        }
        let id = self.0.next_fold_id;
        self.0.next_fold_id = FoldId(
            self.0
                .next_fold_id
                .0
                .checked_add(1)
                .ok_or(FoldError::IdOverflow)?,
        );
        let mut folds: Vec<_> = self.0.snapshot.folds.iter().copied().collect();
        let fold = Fold::new(
            id,
            self.0.snapshot.buffer_snapshot.version(),
            range,
            self.0.default_update_policy,
            fold_line_span(&self.0.snapshot.buffer_snapshot, range)?,
        );
        folds.push(fold);
        sort_folds(&mut folds);
        let old_rows = self.0.snapshot.line_count();
        self.0.snapshot.folds = SumTree::from_iter(folds, ());
        self.0.snapshot.fold_metadata_by_id.insert(id, range);
        let spans = hidden_spans(&self.0.snapshot.folds);
        self.0.snapshot.transforms =
            build_transforms(&spans, self.0.snapshot.buffer_snapshot.line_count());
        self.0.snapshot.version += 1;
        let edit = full_fold_edit(old_rows, self.0.snapshot.line_count());
        Ok((self.0.snapshot.clone(), vec![edit]))
    }

    fn unfold(&mut self, id: FoldId) -> (FoldSnapshot, Vec<FoldEdit>) {
        if !self.0.snapshot.fold_metadata_by_id.contains_key(&id) {
            return (self.0.snapshot.clone(), Vec::new());
        }
        let old_rows = self.0.snapshot.line_count();
        let retained: Vec<_> = self
            .0
            .snapshot
            .folds
            .iter()
            .copied()
            .filter(|fold| fold.id != id)
            .collect();
        self.0.snapshot.folds = SumTree::from_iter(retained, ());
        self.0.snapshot.fold_metadata_by_id.remove(&id);
        let spans = hidden_spans(&self.0.snapshot.folds);
        self.0.snapshot.transforms =
            build_transforms(&spans, self.0.snapshot.buffer_snapshot.line_count());
        self.0.snapshot.version += 1;
        let edit = full_fold_edit(old_rows, self.0.snapshot.line_count());
        (self.0.snapshot.clone(), vec![edit])
    }
}

fn sort_folds(folds: &mut [Fold]) {
    folds.sort_by_key(|fold| FoldOrder::for_fold(*fold));
}

fn hidden_spans(folds: &SumTree<Fold>) -> Vec<Range<usize>> {
    let mut spans: Vec<Range<usize>> = Vec::new();
    for fold in folds.iter() {
        let (start, end) = fold.line_span;
        if start >= end {
            continue;
        }
        let span = start.get() + 1..end.get() + 1;
        match spans.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => spans.push(span),
        }
    }
    spans
}

fn build_transforms(spans: &[Range<usize>], line_count: usize) -> SumTree<Transform> {
    let mut transforms = Vec::new();
    let mut line = 0;
    for span in spans {
        if line < span.start {
            transforms.push(Transform::isomorphic(span.start - line));
        }
        transforms.push(Transform::fold(span.end - span.start));
        line = span.end;
    }
    if line < line_count {
        transforms.push(Transform::isomorphic(line_count - line));
    }
    SumTree::from_iter(transforms, ())
}

fn full_fold_edit(old_rows: usize, new_rows: usize) -> FoldEdit {
    FoldEdit {
        old: ProjectedLineIndex::ZERO..ProjectedLineIndex::new(old_rows),
        new: ProjectedLineIndex::ZERO..ProjectedLineIndex::new(new_rows),
        changed_lines: Vec::new(),
        structural: true,
    }
}

fn inline_fold_edits(batch: &TextChangeBatch, snapshot: &Snapshot) -> Vec<FoldEdit> {
    batch
        .patch()
        .edits()
        .iter()
        .filter_map(|edit| {
            let start = snapshot.byte_to_line(edit.new_range().start()).ok()?;
            let end = snapshot.byte_to_line(edit.new_range().end()).ok()?;
            let changed_lines = (start.get()..=end.get()).map(Line::new).collect();
            Some(FoldEdit {
                old: ProjectedLineIndex::ZERO..ProjectedLineIndex::ZERO,
                new: ProjectedLineIndex::ZERO..ProjectedLineIndex::ZERO,
                changed_lines,
                structural: false,
            })
        })
        .collect()
}

#[cfg(test)]
fn text_range_for_lines(snapshot: &Snapshot, lines: LineRange) -> DisplayMapResult<TextRange> {
    let start = line_boundary(snapshot, lines.start())?;
    let end = line_boundary(snapshot, lines.end())?;
    Ok(TextRange::new(start, end)?)
}

#[cfg(test)]
fn line_boundary(snapshot: &Snapshot, line: Line) -> DisplayMapResult<ByteOffset> {
    if line.get() > snapshot.line_count() {
        return Err(CoordinateError::LineOutOfBounds(line).into());
    }
    if line.get() == snapshot.line_count() {
        return Ok(snapshot.len_bytes());
    }
    Ok(snapshot.line_start_byte(line)?)
}

fn fold_line_span(snapshot: &Snapshot, range: TextRange) -> DisplayMapResult<(Line, Line)> {
    let start = snapshot.byte_to_line(range.start())?;
    let mut end = snapshot.byte_to_line(range.end())?;
    if end > start && snapshot.line_start_byte(end)? == range.end() {
        end = Line::new(end.get() - 1);
    }
    Ok((start, end))
}

#[cfg(test)]
fn ranges_disjoint_or_nested(left: TextRange, right: TextRange) -> bool {
    left.end() <= right.start()
        || right.end() <= left.start()
        || (left.start() <= right.start() && right.end() <= left.end())
        || (right.start() <= left.start() && left.end() <= right.end())
}

#[cfg(test)]
mod tests {
    use zcv_engine::{Buffer, BufferConfig};

    use super::*;

    fn line_range(start: usize, end: usize) -> LineRange {
        LineRange::new(Line::new(start), Line::new(end)).unwrap()
    }

    fn text_range(start: usize, end: usize) -> TextRange {
        TextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap()
    }

    #[test]
    fn fold_snapshot_owns_fold_and_transform_trees_and_keeps_old_snapshots_stable() {
        let buffer = Buffer::scratch(
            "anchor\nhidden one\nhidden two\nafter".to_string(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建");
        let (mut map, before) = FoldMap::new(buffer.snapshot());
        let (after, edits) = map
            .write()
            .fold_lines(LineRange::new(Line::ZERO, Line::new(3)).unwrap())
            .unwrap();

        assert_eq!(before.line_count(), 4);
        assert_eq!(after.line_count(), 3);
        assert_eq!(
            before.buffer_snapshot().version(),
            after.buffer_snapshot().version()
        );
        assert_ne!(before.version(), after.version());
        assert_eq!(after.folds.summary().count, 1);
        assert!(edits.iter().all(FoldEdit::is_structural));
    }

    #[test]
    fn fold_writer_rejects_partial_overlap_but_accepts_nesting() {
        let buffer = Buffer::scratch("abcdef".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(buffer.snapshot());
        map.write().fold(text_range(1, 5)).unwrap();
        map.write().fold(text_range(2, 4)).unwrap();

        let error = map.write().fold(text_range(0, 3)).unwrap_err();
        assert!(matches!(
            error,
            super::super::error::DisplayMapError::Fold(FoldError::OverlapWithoutNesting { .. })
        ));
        assert_eq!(map.snapshot.folds.summary().count, 2);
    }

    #[test]
    fn unfolding_outer_fold_reveals_the_nested_transform() {
        let buffer = Buffer::scratch("a\nb\nc\nd\ne".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(buffer.snapshot());
        map.write().fold_lines(line_range(0, 4)).unwrap();
        map.write().fold_lines(line_range(1, 3)).unwrap();
        let outer = map
            .snapshot
            .folds
            .iter()
            .min_by_key(|fold| fold.text_range().start())
            .unwrap()
            .id;

        assert_eq!(map.snapshot.line_count(), 3);
        let (snapshot, edits) = map.write().unfold(outer);
        assert_eq!(snapshot.folds.summary().count, 1);
        assert_eq!(snapshot.line_count(), 5);
        assert!(edits[0].is_structural());
    }

    #[test]
    fn inline_edit_advances_fold_snapshot_without_rebuilding_transforms() {
        let mut buffer =
            Buffer::scratch("anchor\nhidden\nafter".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(buffer.snapshot());
        map.write().fold_lines(line_range(0, 2)).unwrap();
        let transforms = map.snapshot.transforms.clone();
        let subscription = buffer.subscribe();
        buffer.insert(ByteOffset::new(9), "!").unwrap();

        let (snapshot, edits, outcome) = map.read(buffer.snapshot(), &subscription.consume());
        assert_eq!(outcome, ApplyOutcome::Compatible);
        assert_eq!(snapshot.transforms, transforms);
        assert_eq!(snapshot.folds.summary().count, 1);
        assert!(edits.iter().all(|edit| !edit.is_structural()));
    }

    #[test]
    fn newline_edit_rebuilds_transform_tree_and_emits_structural_fold_edit() {
        let mut buffer =
            Buffer::scratch("anchor\nhidden\nafter".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(buffer.snapshot());
        map.write().fold_lines(line_range(0, 2)).unwrap();
        let subscription = buffer.subscribe();
        buffer.insert(ByteOffset::new(9), "new\n").unwrap();

        let (snapshot, edits, outcome) = map.read(buffer.snapshot(), &subscription.consume());
        assert_eq!(outcome, ApplyOutcome::Spliced);
        assert_eq!(
            snapshot.logical_line_count(),
            buffer.snapshot().line_count()
        );
        assert!(edits.iter().all(FoldEdit::is_structural));
    }

    #[test]
    fn deleting_folded_text_invalidates_tracked_fold() {
        let mut buffer =
            Buffer::scratch("anchor\nhidden\nafter".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(buffer.snapshot());
        map.write().fold_lines(line_range(0, 2)).unwrap();
        let subscription = buffer.subscribe();
        buffer
            .delete(text_range(0, "anchor\nhidden\n".len()))
            .unwrap();

        let (snapshot, _, _) = map.read(buffer.snapshot(), &subscription.consume());
        assert_eq!(snapshot.folds.summary().count, 0);
        assert_eq!(
            snapshot.line_count(),
            snapshot.buffer_snapshot().line_count()
        );
    }

    #[test]
    fn folded_points_map_through_placeholder_in_both_directions() {
        let buffer = Buffer::scratch("a\nb\nc\nd".to_string(), BufferConfig::default()).unwrap();
        let (mut map, _) = FoldMap::new(buffer.snapshot());
        let (snapshot, _) = map.write().fold_lines(line_range(0, 3)).unwrap();

        let hidden = snapshot
            .logical_to_projected_point(LogicalPoint::line_start(Line::new(1)))
            .unwrap();
        assert!(matches!(hidden, LogicalPointProjection::Hidden { .. }));
        let placeholder = snapshot
            .projected_to_logical_point(ProjectedPoint::line_start(ProjectedLineIndex::new(1)))
            .unwrap();
        assert!(matches!(
            placeholder,
            ProjectedPointMapping::Placeholder { .. }
        ));
    }
}

/// FoldSnapshot 中投影行的 0-indexed 索引。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ProjectedLineIndex(usize);

impl ProjectedLineIndex {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

/// 投影行的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectedLineKind {
    /// 投影行展示的是某条可见逻辑行。
    Text(TextLine),
    /// 投影行是一段被折叠隐藏内容的占位符。
    Placeholder(FoldPlaceholder),
}

impl ProjectedLineKind {
    pub fn text_line(&self) -> Option<TextLine> {
        match self {
            Self::Text(text_line) => Some(*text_line),
            Self::Placeholder(_) => None,
        }
    }
}

/// FoldSnapshot 中携带索引信息的投影行视图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectedLine {
    index: ProjectedLineIndex,
    kind: ProjectedLineKind,
}

impl ProjectedLine {
    pub(crate) fn new(index: ProjectedLineIndex, kind: ProjectedLineKind) -> Self {
        Self { index, kind }
    }

    pub fn kind(self) -> ProjectedLineKind {
        self.kind
    }
}

/// 可见逻辑行投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextLine {
    logical_line: Line,
}

impl TextLine {
    pub fn new(logical_line: Line) -> Self {
        Self { logical_line }
    }

    pub fn logical_line(self) -> Line {
        self.logical_line
    }
}

/// 折叠占位符投影行。
///
/// `anchor_line` 是该占位符紧跟其后的可见 anchor 逻辑行；`hidden_lines` 是被折叠隐藏的
/// 半开行区间 `[first_hidden, end_exclusive)`。当多个 fold 折叠的逻辑行区间合并为一段连续
/// 的隐藏区间时，引擎只产出一条占位符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FoldPlaceholder {
    anchor_line: Line,
    hidden_lines: LineRange,
}

impl FoldPlaceholder {
    pub(crate) fn new(anchor_line: Line, hidden_lines: LineRange) -> Self {
        Self {
            anchor_line,
            hidden_lines,
        }
    }

    pub fn anchor_line(self) -> Line {
        self.anchor_line
    }

    pub fn hidden_lines(self) -> LineRange {
        self.hidden_lines
    }
}

/// 逻辑行 -> 投影空间的查询结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogicalProjection {
    /// 逻辑行可见，对应投影行索引。
    Visible(ProjectedLineIndex),
    /// 逻辑行被某段 fold 隐藏；返回该 fold 的 anchor 信息。
    Hidden {
        /// 隐藏该逻辑行的 fold anchor（该 fold 第一条仍可见的逻辑行）。
        anchor_logical_line: Line,
        /// anchor 在投影空间的索引；可作为「跳到 fold 起点」的目标。
        anchor_projected_line: ProjectedLineIndex,
    },
}

/// 逻辑文档内的 (line, column) 点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct LogicalPoint {
    pub line: Line,
    pub column: LogicalColumn,
}

impl LogicalPoint {
    pub const fn new(line: Line, column: LogicalColumn) -> Self {
        Self { line, column }
    }

    pub const fn line_start(line: Line) -> Self {
        Self {
            line,
            column: LogicalColumn::ZERO,
        }
    }

    pub const fn line(self) -> Line {
        self.line
    }

    pub const fn column(self) -> LogicalColumn {
        self.column
    }

    pub fn into_position(self) -> Position {
        Position::new(self.line, self.column)
    }
}

impl From<Position> for LogicalPoint {
    fn from(position: Position) -> Self {
        Self {
            line: position.line(),
            column: position.column(),
        }
    }
}

impl From<LogicalPoint> for Position {
    fn from(point: LogicalPoint) -> Self {
        point.into_position()
    }
}

/// 投影空间内的 (projected_line, column) 点。
///
/// 当 `line` 指向一条 `TextLine` 时 `column` 与对应逻辑行的 `LogicalColumn` 同义；
/// 当 `line` 指向一条 `FoldPlaceholder` 时 `column` 没有逻辑文本意义，由宿主决定如何使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ProjectedPoint {
    pub line: ProjectedLineIndex,
    pub column: LogicalColumn,
}

impl ProjectedPoint {
    pub const fn new(line: ProjectedLineIndex, column: LogicalColumn) -> Self {
        Self { line, column }
    }

    pub const fn line_start(line: ProjectedLineIndex) -> Self {
        Self {
            line,
            column: LogicalColumn::ZERO,
        }
    }

    pub const fn line(self) -> ProjectedLineIndex {
        self.line
    }

    pub const fn column(self) -> LogicalColumn {
        self.column
    }
}

/// `LogicalPoint` -> 投影空间的查询结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogicalPointProjection {
    /// 逻辑点所在行可见，直接对应一个投影点。
    Visible(ProjectedPoint),
    /// 逻辑点所在行被某段 fold 隐藏；返回 fold anchor 的逻辑点与投影点。
    Hidden {
        anchor_logical: LogicalPoint,
        anchor_projected: ProjectedPoint,
    },
}

/// `ProjectedPoint` -> 逻辑空间的查询结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectedPointMapping {
    /// 投影点所在行是 `TextLine`，对应单一逻辑点。
    Text(LogicalPoint),
    /// 投影点所在行是 `FoldPlaceholder`，返回 fold anchor 与该 placeholder 覆盖的隐藏行区间。
    Placeholder {
        anchor: LogicalPoint,
        hidden_lines: LineRange,
    },
}

/// 逻辑文档内的有序点对范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LogicalRange {
    start: LogicalPoint,
    end: LogicalPoint,
}

impl LogicalRange {
    /// 要求 `start <= end`（按 line, column 字典序）。
    pub fn new(start: LogicalPoint, end: LogicalPoint) -> Result<Self, CoordinateError> {
        if !is_ordered_logical(start, end) {
            return Err(CoordinateError::InvalidLineRange {
                start: start.line,
                end: end.line,
            });
        }
        Ok(Self { start, end })
    }

    pub const fn start(self) -> LogicalPoint {
        self.start
    }

    pub const fn end(self) -> LogicalPoint {
        self.end
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// 投影空间内的有序点对范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectedRange {
    start: ProjectedPoint,
    end: ProjectedPoint,
}

impl ProjectedRange {
    /// 要求 `start <= end`（按 projected line, column 字典序）。
    pub fn new(start: ProjectedPoint, end: ProjectedPoint) -> Result<Self, CoordinateError> {
        if !is_ordered_projected(start, end) {
            return Err(CoordinateError::InvalidLineRange {
                start: Line::new(start.line.get()),
                end: Line::new(end.line.get()),
            });
        }
        Ok(Self { start, end })
    }

    pub const fn start(self) -> ProjectedPoint {
        self.start
    }

    pub const fn end(self) -> ProjectedPoint {
        self.end
    }
}

fn is_ordered_logical(start: LogicalPoint, end: LogicalPoint) -> bool {
    if start.line < end.line {
        return true;
    }
    if start.line == end.line {
        return start.column <= end.column;
    }
    false
}

fn is_ordered_projected(start: ProjectedPoint, end: ProjectedPoint) -> bool {
    if start.line < end.line {
        return true;
    }
    if start.line == end.line {
        return start.column <= end.column;
    }
    false
}
