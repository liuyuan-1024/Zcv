//! 插入索引：文本位置到稳定文档序身份的映射。
//!
//! 对齐 Zed 的 insertion fragments + Locator + UndoMap：文本按「插入」划分为稳定身份，每次插入分配一个 Locator；
//! 编辑只切分既有插入并记录删除操作，不重编号已有 Locator。
//! 片段可见性由每个片段的插入/删除操作与撤销计数推导，undo/redo 只切换撤销计数，不重写片段状态。
//! 因此以 (插入身份, 插入内偏移) 表示的 Anchor 顺序在编辑前后保持稳定，比较不再需要解析文本坐标。

use std::collections::{BTreeMap, BTreeSet};

use sum_tree::{Bias, ContextLessSummary, Dimension, Item, SumTree};

use super::locator::Locator;
use crate::transaction::EditList;
use crate::types::BufferVersion;

/// 一次插入的稳定身份；0 保留给空文档。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub(crate) struct InsertionId(u64);

impl InsertionId {
    pub(crate) const NONE: Self = Self(0);

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// 插入内的一段；切分后同一插入会有多段。
///
/// `inserted_at` 是本次插入的操作版本，`deletions` 是删除过该片段的操作版本。
/// 片段可见性由二者与 `undos` 推导，undo 不重写片段状态（对齐 Zed 的 Fragment + UndoMap）。
#[derive(Clone, Debug)]
struct Piece {
    base: u32,
    len: u32,
    locator: Locator,
    inserted_at: BufferVersion,
    deletions: Vec<BufferVersion>,
}

/// 按位置排序的可见插入段。
#[derive(Clone, Debug)]
struct Run {
    id: InsertionId,
    base: u32,
    len: u32,
    locator: Locator,
}

#[derive(Clone, Debug, Default)]
struct RunSummary {
    len: u32,
    max_locator: Locator,
}

impl ContextLessSummary for RunSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, other: &Self) {
        self.len += other.len;
        if other.max_locator > self.max_locator {
            self.max_locator = other.max_locator.clone();
        }
    }
}

impl Item for Run {
    type Summary = RunSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        RunSummary {
            len: self.len,
            max_locator: self.locator.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct RunLen(u32);

impl<'a> Dimension<'a, RunSummary> for RunLen {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a RunSummary, (): ()) {
        self.0 += summary.len;
    }
}

/// 文本的稳定插入身份索引。
#[derive(Clone, Debug, Default)]
pub(crate) struct InsertionIndex {
    insertions: BTreeMap<u64, Vec<Piece>>,
    runs: SumTree<Run>,
    next_id: u64,
    /// 每个操作版本的撤销次数记录；奇偶决定该操作是否被撤销（对齐 Zed 的 UndoMap）。
    undos: BTreeMap<BufferVersion, Vec<BufferVersion>>,
}

/// 一个位置解析出的稳定身份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InsertionPosition {
    pub(crate) id: InsertionId,
    pub(crate) offset: u32,
}

impl InsertionIndex {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 为初始全文建立一段稳定插入身份；空文本保持未绑定。
    pub(crate) fn with_text(len: usize) -> Self {
        let mut index = Self::new();
        if len > 0 {
            index.apply_edit(0, 0, len, BufferVersion::INITIAL);
        }
        index
    }

    /// 当前文本位置对应的稳定身份；空文档返回 None。
    pub(crate) fn position_at(&self, offset: usize) -> Option<InsertionPosition> {
        if self.runs.is_empty() {
            return None;
        }
        let target = RunLen(u32::try_from(offset).ok()?);
        let mut cursor = self.runs.cursor::<RunLen>(());
        cursor.seek(&target, Bias::Right);
        let mut run = cursor.item()?;
        let mut start = cursor.start().0;
        // 位置落在某段末尾时归属后一段（半开区间归后）。
        if target.0 >= start + run.len {
            cursor.next();
            run = cursor.item()?;
            start = cursor.start().0;
        }
        let overshoot = offset as u32 - start;
        Some(InsertionPosition {
            id: run.id,
            offset: run.base + overshoot,
        })
    }

    /// 稳定身份对应的 Locator；身份已不存在时返回 None。
    pub(crate) fn locator_of(&self, id: InsertionId, offset: u32) -> Option<&Locator> {
        let pieces = self.insertions.get(&id.get())?;
        let index = pieces
            .partition_point(|piece| piece.base <= offset)
            .checked_sub(1)?;
        let piece = pieces.get(index)?;
        (offset < piece.base + piece.len).then_some(&piece.locator)
    }

    /// 应用一次编辑列表，返回推进后的索引。
    pub(crate) fn with_edits(&self, edits: &EditList, version: BufferVersion) -> Self {
        let mut next = self.clone();
        for edit in edits.as_slice() {
            next.apply_edit(
                edit.range().start().get(),
                edit.range().end().get(),
                edit.replacement().len(),
                version,
            );
        }
        next
    }

    /// 切换 `[start, end]` 版本区间内操作的撤销计数，并重建位置索引。
    ///
    /// 对齐 Zed 的 UndoMap：undo/redo 都是撤销操作，按奇偶切换对应操作的撤销次数；
    /// 片段可见性由 `deletions` 与 `undos` 推导，不重写片段状态。
    pub(crate) fn undone(
        &self,
        start: BufferVersion,
        end: BufferVersion,
        version: BufferVersion,
    ) -> Self {
        let mut next = self.clone();
        let mut toggled: BTreeSet<BufferVersion> = BTreeSet::new();
        for piece in next.insertions.values().flatten() {
            if piece.inserted_at > start && piece.inserted_at <= end {
                toggled.insert(piece.inserted_at);
            }
            for deletion in &piece.deletions {
                if *deletion > start && *deletion <= end {
                    toggled.insert(*deletion);
                }
            }
        }
        for edit in toggled {
            next.undos.entry(edit).or_default().push(version);
        }
        next.rebuild_runs();
        next
    }

    /// 当前撤销计数下该操作是否处于已撤销状态。
    fn is_undone(&self, edit: BufferVersion) -> bool {
        self.undos
            .get(&edit)
            .is_some_and(|undos| undos.len() % 2 == 1)
    }

    /// `version` 观察到该操作的撤销次数是否为奇数。
    fn was_undone(&self, edit: BufferVersion, version: BufferVersion) -> bool {
        self.undos
            .get(&edit)
            .is_some_and(|undos| undos.iter().filter(|undo| **undo <= version).count() % 2 == 1)
    }

    /// 片段在当前撤销状态下的可见性。
    fn piece_is_visible(&self, piece: &Piece) -> bool {
        !self.is_undone(piece.inserted_at)
            && piece
                .deletions
                .iter()
                .all(|deletion| self.is_undone(*deletion))
    }

    /// 片段在 `version` 时的可见性，对齐 Zed 的 Fragment::was_visible。
    fn piece_was_visible(&self, piece: &Piece, version: BufferVersion) -> bool {
        version >= piece.inserted_at
            && !self.was_undone(piece.inserted_at, version)
            && piece
                .deletions
                .iter()
                .all(|deletion| version < *deletion || self.was_undone(*deletion, version))
    }

    /// 由全部片段重建按位置排序的可见段索引。
    fn rebuild_runs(&mut self) {
        let mut visible: Vec<Run> = Vec::new();
        for (id, pieces) in &self.insertions {
            for piece in pieces {
                if self.piece_is_visible(piece) {
                    visible.push(Run {
                        id: InsertionId(*id),
                        base: piece.base,
                        len: piece.len,
                        locator: piece.locator.clone(),
                    });
                }
            }
        }
        visible.sort_by(|left, right| {
            left.locator
                .cmp(&right.locator)
                .then_with(|| left.base.cmp(&right.base))
        });
        self.runs = SumTree::from_iter(visible, ());
    }

    /// `since` 到当前之间，可见片段集合是否发生变化。
    ///
    /// 对齐 Zed 的 `BufferSnapshot::has_edits_since`：逐片段比较「在 since 时是否可见」与「现在是否可见」。
    pub(crate) fn has_edits_since(&self, since: BufferVersion) -> bool {
        self.insertions
            .values()
            .flatten()
            .any(|piece| self.piece_was_visible(piece, since) != self.piece_is_visible(piece))
    }

    fn apply_edit(
        &mut self,
        start: usize,
        end: usize,
        replacement_len: usize,
        version: BufferVersion,
    ) {
        let replacement_len = u32::try_from(replacement_len).expect("替换文本长度必须适配 u32");
        let start = u32::try_from(start).expect("文本长度必须适配 u32");
        let end = u32::try_from(end).expect("文本长度必须适配 u32");
        let old_runs = std::mem::take(&mut self.runs);
        let mut new_runs = SumTree::<Run>::new(());
        let mut cursor = old_runs.cursor::<RunLen>(());
        new_runs.append(cursor.slice(&RunLen(start), Bias::Right), ());

        // 起点落在某段内部时切分：左半重新编号，右半保留原 Locator，保证后续插入落在二者之间。
        if let Some(run) = cursor.item().cloned() {
            let run_start = cursor.start().0;
            let run_end = run_start + run.len;
            if run_start < start && start < run_end {
                let left_len = start - run_start;
                let left_locator = Locator::between(&new_runs.summary().max_locator, &run.locator);
                self.split_piece(run.id, run.base, run.len, left_len, left_locator.clone());
                new_runs.push(
                    Run {
                        id: run.id,
                        base: run.base,
                        len: left_len,
                        locator: left_locator,
                    },
                    (),
                );
            }
        }

        // 插入点两侧的稳定 Locator：插入文本排在左侧之后、右侧之前。
        let left_locator = new_runs.summary().max_locator.clone();
        let right_locator = cursor
            .item()
            .map_or_else(Locator::max, |run| run.locator.clone());

        if replacement_len > 0 {
            self.next_id += 1;
            let id = InsertionId(self.next_id);
            let locator = Locator::between(&left_locator, &right_locator);
            self.insertions.insert(
                id.get(),
                vec![Piece {
                    base: 0,
                    len: replacement_len,
                    locator: locator.clone(),
                    inserted_at: version,
                    deletions: Vec::new(),
                }],
            );
            new_runs.push(
                Run {
                    id,
                    base: 0,
                    len: replacement_len,
                    locator,
                },
                (),
            );
        }

        // 消费 [start, end) 覆盖的可见段；起点所在段的右半在这里补出。
        while let Some(run) = cursor.item().cloned() {
            let raw_start = cursor.start().0;
            let run_end = raw_start + run.len;
            let effective_start = raw_start.max(start);
            let effective_base = run.base + (effective_start - raw_start);
            if effective_start >= end {
                if raw_start < start {
                    // 起点所在段的右半仍可见：发出后前进，避免被 suffix 重复搬运。
                    new_runs.push(
                        Run {
                            id: run.id,
                            base: effective_base,
                            len: run_end - effective_start,
                            locator: run.locator.clone(),
                        },
                        (),
                    );
                    cursor.next();
                }
                break;
            }
            if run_end <= end {
                self.mark_deleted(run.id, effective_base, run_end - effective_start, version);
                cursor.next();
                continue;
            }
            // 跨过终点：前半删除，后半保留。
            let deleted_len = end - effective_start;
            self.mark_deleted(run.id, effective_base, deleted_len, version);
            new_runs.push(
                Run {
                    id: run.id,
                    base: effective_base + deleted_len,
                    len: run_end - end,
                    locator: run.locator.clone(),
                },
                (),
            );
            cursor.next();
            break;
        }

        new_runs.append(cursor.suffix(), ());
        drop(cursor);
        self.runs = new_runs;
    }

    /// 把插入的一段按 left_len 切成「重新编号的左半 + 保留 Locator 的右半」。
    fn split_piece(&mut self, id: InsertionId, base: u32, len: u32, left_len: u32, left: Locator) {
        let Some(pieces) = self.insertions.get_mut(&id.get()) else {
            return;
        };
        let Some(index) = pieces
            .iter()
            .position(|piece| piece.base == base && piece.len == len)
        else {
            return;
        };
        let original = pieces.remove(index);
        let right_len = len - left_len;
        pieces.insert(
            index,
            Piece {
                base,
                len: left_len,
                locator: left,
                inserted_at: original.inserted_at,
                deletions: original.deletions.clone(),
            },
        );
        if right_len > 0 {
            pieces.insert(
                index + 1,
                Piece {
                    base: base + left_len,
                    len: right_len,
                    locator: original.locator,
                    inserted_at: original.inserted_at,
                    deletions: original.deletions,
                },
            );
        }
    }

    /// 把 [base, base+len) 记为在 `version` 删除；跨边界的片段按需切分，只标记相交部分。
    fn mark_deleted(&mut self, id: InsertionId, base: u32, len: u32, version: BufferVersion) {
        let Some(pieces) = self.insertions.get_mut(&id.get()) else {
            return;
        };
        let deleted_end = base + len;
        let mut split = Vec::with_capacity(pieces.len() + 2);
        for piece in pieces.drain(..) {
            let piece_end = piece.base + piece.len;
            if piece_end <= base || piece.base >= deleted_end {
                split.push(piece);
                continue;
            }
            let overlap_start = piece.base.max(base);
            let overlap_end = piece_end.min(deleted_end);
            if piece.base < overlap_start {
                split.push(Piece {
                    base: piece.base,
                    len: overlap_start - piece.base,
                    locator: piece.locator.clone(),
                    inserted_at: piece.inserted_at,
                    deletions: piece.deletions.clone(),
                });
            }
            let mut deletions = piece.deletions.clone();
            deletions.push(version);
            split.push(Piece {
                base: overlap_start,
                len: overlap_end - overlap_start,
                locator: piece.locator.clone(),
                inserted_at: piece.inserted_at,
                deletions,
            });
            if overlap_end < piece_end {
                split.push(Piece {
                    base: overlap_end,
                    len: piece_end - overlap_end,
                    locator: piece.locator,
                    inserted_at: piece.inserted_at,
                    deletions: piece.deletions,
                });
            }
        }
        *pieces = split;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{Edit, EditList};
    use crate::types::{ByteOffset, TextRange};

    fn edit_list(edits: Vec<Edit>) -> EditList {
        EditList::new(edits).expect("测试编辑必须合法")
    }

    fn replace(range: std::ops::Range<usize>, text: &str) -> Edit {
        Edit::new(
            TextRange::new(ByteOffset::new(range.start), ByteOffset::new(range.end)).unwrap(),
            text,
        )
    }

    fn position(index: &InsertionIndex, offset: usize) -> InsertionPosition {
        index.position_at(offset).expect("位置必须存在")
    }

    fn apply(mut index: InsertionIndex, edits: Vec<Edit>, version: u64) -> InsertionIndex {
        index = index.with_edits(&edit_list(edits), BufferVersion::new(version));
        index
    }

    #[test]
    fn sequential_insertions_keep_stable_order() {
        let mut index = InsertionIndex::new();
        index = apply(index, vec![replace(0..0, "abc")], 1);
        index = apply(index, vec![replace(3..3, "def")], 2);
        index = apply(index, vec![replace(0..0, "xy")], 3);
        assert!(index.position_at(7).is_some());
        assert!(index.position_at(8).is_none());

        let first = position(&index, 0);
        let second = position(&index, 2);
        let third = position(&index, 5);
        assert!(
            index.locator_of(first.id, first.offset) < index.locator_of(second.id, second.offset)
        );
        assert!(
            index.locator_of(second.id, second.offset) < index.locator_of(third.id, third.offset)
        );
    }

    #[test]
    fn insertion_inside_a_run_sorts_between_split_halves() {
        let mut index = InsertionIndex::new();
        index = apply(index, vec![replace(0..0, "abcdef")], 1);
        index = apply(index, vec![replace(3..3, "XY")], 2);
        assert!(index.position_at(7).is_some());
        assert!(index.position_at(8).is_none());

        let before = position(&index, 0);
        let inserted = position(&index, 3);
        let after = position(&index, 5);
        assert!(
            index.locator_of(before.id, before.offset)
                < index.locator_of(inserted.id, inserted.offset)
        );
        assert!(
            index.locator_of(inserted.id, inserted.offset)
                < index.locator_of(after.id, after.offset)
        );
    }

    #[test]
    fn deletion_keeps_identity_of_surviving_text() {
        let mut index = InsertionIndex::new();
        index = apply(index, vec![replace(0..0, "abcdef")], 1);
        let before = position(&index, 0);
        let surviving = position(&index, 5);
        index = apply(index, vec![replace(1..3, "")], 2);
        assert!(index.position_at(3).is_some());
        assert!(index.position_at(4).is_none());
        let after = position(&index, 3);
        assert_eq!(surviving.id, after.id);
        assert!(
            index.locator_of(before.id, before.offset) < index.locator_of(after.id, after.offset)
        );
    }

    #[test]
    fn visibility_changes_match_fragment_semantics() {
        let base = InsertionIndex::with_text("hello".len());
        assert!(!base.has_edits_since(BufferVersion::INITIAL));

        // 插入新片段：相对插入前是编辑，相对插入后不是。
        let inserted = apply(base.clone(), vec![replace(5..5, "!")], 1);
        assert!(inserted.has_edits_since(BufferVersion::INITIAL));
        assert!(!inserted.has_edits_since(BufferVersion::new(1)));

        // 插入后删除：片段在 INITIAL 时不可见、现在也不可见，判为无编辑；相对 v1 则判为有编辑。
        let removed = apply(inserted, vec![replace(5..6, "")], 2);
        assert!(!removed.has_edits_since(BufferVersion::INITIAL));
        assert!(removed.has_edits_since(BufferVersion::new(1)));

        // 删除既有文本：片段从可见变为不可见，判为有编辑。
        let deleted = apply(base, vec![replace(0..1, "")], 3);
        assert!(deleted.has_edits_since(BufferVersion::INITIAL));
    }
}
