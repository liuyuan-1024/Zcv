//! 版本索引的编辑日志：Buffer 拥有的版本化编辑事实。
//!
//! 它同时是 `edits_since` 增量同步与 undo/redo 回放的唯一索引：
//! 每个版本条目保存该步的向前编辑（旧文本坐标）与逆编辑（新文本坐标）；
//! 逆编辑只在事务进入历史时保留，避免 SkipHistory 大事务白存被删文本。
//!
//! 历史图只引用版本区间，不再复制 `EditList`。

use std::ops::Range;

use sum_tree::{Bias, ContextLessSummary, Dimension, Item, SumTree};

use crate::{
    BufferVersion, TextError, TextRange, TextResult,
    text_changes::{TextChangeBatch, TextPatch},
    transaction::EditList,
};

/// 一次已提交版本推进的编辑事实。
#[derive(Debug, Clone)]
struct VersionedEdit {
    old_version: BufferVersion,
    new_version: BufferVersion,
    /// 向前编辑，坐标以旧文本为基准；供 `edits_since` 与 redo 使用。
    forward: EditList,
    /// 逆编辑，坐标以新文本为基准；仅已记录历史的事务保留，供 undo 使用。
    undo: Option<EditList>,
}

impl VersionedEdit {
    fn replacement_bytes(&self) -> usize {
        self.forward.replacement_bytes() + self.undo.as_ref().map_or(0, EditList::replacement_bytes)
    }
}

#[derive(Clone, Debug, Default)]
struct EditSummary {
    count: usize,
    replacement_bytes: usize,
}

impl ContextLessSummary for EditSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, other: &Self) {
        self.count += other.count;
        self.replacement_bytes += other.replacement_bytes;
    }
}

impl Item for VersionedEdit {
    type Summary = EditSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        EditSummary {
            count: 1,
            replacement_bytes: self.replacement_bytes(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct EntryCount(usize);

impl<'a> Dimension<'a, EditSummary> for EntryCount {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a EditSummary, (): ()) {
        self.0 += summary.count;
    }
}

/// 单调版本索引的不可变编辑日志。
///
/// 每次提交只追加新条目并共享旧树节点，旧 Snapshot 继续看到自己的版本区间；
/// 超出编辑历史预算的条目从最老端裁剪，不影响独立的长期锚点坐标索引。
#[derive(Debug, Clone, Default)]
pub(crate) struct EditLog {
    entries: SumTree<VersionedEdit>,
}

impl EditLog {
    /// 追加一次版本推进。
    pub(crate) fn appended(
        &self,
        old_version: BufferVersion,
        new_version: BufferVersion,
        forward: EditList,
        undo: Option<EditList>,
    ) -> Self {
        let mut next = self.clone();
        if let Some(last) = next.entries.last() {
            assert_eq!(last.new_version, old_version, "编辑日志版本必须连续");
        }
        assert_eq!(old_version.next(), Some(new_version), "版本必须推进一步");
        next.entries.push(
            VersionedEdit {
                old_version,
                new_version,
                forward,
                undo,
            },
            (),
        );
        next
    }

    /// 最早保留版本的起点偏移；空日志返回 None。
    pub(crate) fn earliest_version(&self) -> Option<BufferVersion> {
        self.entries.first().map(|entry| entry.old_version)
    }

    /// 按条目数与字节预算从最老端裁剪。
    ///
    /// `max_entries == 0` 清空日志；`max_bytes == 0` 表示不限制字节，
    /// 只按条目数裁剪。最新一个条目无论多大都保留，否则当前版本的增量同步会立即失效。
    pub(crate) fn truncated(&self, max_entries: usize, max_bytes: usize) -> Self {
        if max_entries == 0 {
            return Self::default();
        }
        let summary = self.entries.summary();
        if summary.count <= 1
            || (summary.count <= max_entries
                && (max_bytes == 0 || summary.replacement_bytes <= max_bytes))
        {
            return self.clone();
        }

        let mut remaining_count = summary.count;
        let mut remaining_bytes = summary.replacement_bytes;
        let mut cursor = self.entries.cursor::<EntryCount>(());
        cursor.next();
        while remaining_count > 1
            && (remaining_count > max_entries || (max_bytes != 0 && remaining_bytes > max_bytes))
        {
            let entry = cursor.item().expect("裁剪时必须存在最老条目");
            remaining_count -= 1;
            remaining_bytes -= entry.replacement_bytes();
            cursor.next();
        }
        Self {
            entries: cursor.suffix(),
        }
    }

    /// 组合 since 到 current 之间的连续编辑为单个批次。
    ///
    /// since == current 返回空批次；
    /// since 已被裁剪或不在本日志中时返回 `TextError::VersionEvicted`，调用方必须回退而不是猜测坐标。
    pub(crate) fn batch_since(
        &self,
        since: BufferVersion,
        current: BufferVersion,
    ) -> TextResult<TextChangeBatch> {
        if since == current {
            return Ok(TextChangeBatch::default());
        }
        let range = self.entries_for_range(since, current)?;

        let mut patch = TextPatch::default();
        for entry in self.iter_range(range) {
            patch = patch.compose(&TextPatch::from_edit_list(entry.forward.as_slice()));
        }
        Ok(TextChangeBatch::from_patch(since, current, patch))
    }

    /// 组合 since 到 current 之间与 range 相交的编辑。
    pub(crate) fn batch_since_in_range(
        &self,
        since: BufferVersion,
        current: BufferVersion,
        range: TextRange,
    ) -> TextResult<TextChangeBatch> {
        let batch = self.batch_since(since, current)?;
        Ok(batch.filtered_to_old_range(range))
    }

    /// 取 `[start, end]` 版本区间的逆编辑与原始版本区间，按 undo 回放顺序（版本倒序）返回。
    ///
    /// 原始版本区间供插入索引回退片段可见性，对齐 Zed 的 undo map。
    pub(crate) fn undo_batches(
        &self,
        start: BufferVersion,
        end: BufferVersion,
    ) -> TextResult<Vec<(BufferVersion, BufferVersion, EditList)>> {
        let range = self.entries_for_range(start, end)?;
        let mut batches = Vec::with_capacity(range.len());
        let mut cursor = self.entries.cursor::<EntryCount>(());
        cursor.seek(&EntryCount(range.end), Bias::Right);
        cursor.prev();
        for _ in range {
            let entry = cursor.item().expect("回放区间必须存在编辑条目");
            let Some(undo) = &entry.undo else {
                return Err(TextError::InvariantViolation {
                    location: "EditLog::undo_batches",
                    detail: "历史节点引用了未保留逆编辑的版本".to_string(),
                });
            };
            batches.push((entry.old_version, entry.new_version, undo.clone()));
            cursor.prev();
        }
        Ok(batches)
    }

    /// 取 `[start, end]` 版本区间的逆编辑，按版本倒序返回，供按旧版本重建文本。
    ///
    /// 与 undo 回放不同：重建历史文本允许区间内存在未保留逆编辑的事务（放弃历史的大事务），
    /// 此时返回显式错误，调用方必须丢弃而不是猜测文本。
    pub(crate) fn reverse_batches(
        &self,
        start: BufferVersion,
        end: BufferVersion,
    ) -> TextResult<Vec<EditList>> {
        let range = self.entries_for_range(start, end)?;
        let mut batches = Vec::with_capacity(range.len());
        let mut cursor = self.entries.cursor::<EntryCount>(());
        cursor.seek(&EntryCount(range.end), Bias::Right);
        cursor.prev();
        for _ in range {
            let entry = cursor.item().expect("历史区间必须存在编辑条目");
            let Some(undo) = &entry.undo else {
                return Err(TextError::HistoryTextUnavailable {
                    requested: start,
                    current: end,
                });
            };
            batches.push(undo.clone());
            cursor.prev();
        }
        Ok(batches)
    }

    /// 取 `[start, end]` 版本区间的向前编辑与原始版本区间，按 redo 回放顺序（版本正序）返回。
    pub(crate) fn redo_batches(
        &self,
        start: BufferVersion,
        end: BufferVersion,
    ) -> TextResult<Vec<(BufferVersion, BufferVersion, EditList)>> {
        let range = self.entries_for_range(start, end)?;
        Ok(self
            .iter_range(range)
            .map(|entry| (entry.old_version, entry.new_version, entry.forward.clone()))
            .collect())
    }

    /// `[start, end]` 连续版本区间内的每个条目是否都保留了逆编辑。
    ///
    /// 空区间恒为 true。区间不连续、已退出日志或存在放弃历史的条目时返回 false，
    /// 调用方据此拒绝把不可回放的版本区间并入历史节点。
    pub(crate) fn range_is_replayable(&self, start: BufferVersion, end: BufferVersion) -> bool {
        if start == end {
            return true;
        }
        self.entries_for_range(start, end)
            .is_ok_and(|range| self.iter_range(range).all(|entry| entry.undo.is_some()))
    }

    /// 定位 `[start, end]` 的连续版本区间。
    fn entries_for_range(
        &self,
        start: BufferVersion,
        end: BufferVersion,
    ) -> TextResult<Range<usize>> {
        let Some(first) = self.entries.first() else {
            return Err(self.evicted(start, end));
        };
        let last = self.entries.last().expect("非空日志必须有最后条目");
        if start < first.old_version || end > last.new_version || start >= end {
            return Err(self.evicted(start, end));
        }
        let start_index = usize::try_from(start.get() - first.old_version.get())
            .map_err(|_| self.evicted(start, end))?;
        let end_index = usize::try_from(end.get() - first.old_version.get())
            .map_err(|_| self.evicted(start, end))?;
        if end_index > self.entries.summary().count {
            return Err(self.evicted(start, end));
        }
        Ok(start_index..end_index)
    }

    fn iter_range(&self, range: Range<usize>) -> impl Iterator<Item = &VersionedEdit> + '_ {
        let mut cursor = self.entries.cursor::<EntryCount>(());
        cursor.seek(&EntryCount(range.start), Bias::Right);
        cursor.take(range.len())
    }

    fn evicted(&self, requested: BufferVersion, current: BufferVersion) -> TextError {
        TextError::VersionEvicted {
            requested,
            earliest: self
                .entries
                .first()
                .map_or(current, |entry| entry.old_version),
        }
    }
}

#[cfg(test)]
#[path = "test/edit_log_tests.rs"]
mod tests;
