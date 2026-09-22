//! 版本索引的编辑日志：Buffer 拥有的版本化编辑事实。
//!
//! 它同时是 `edits_since` 增量同步与 undo/redo 回放的唯一索引：
//! 每个版本条目保存该步的向前编辑（旧文本坐标）与逆编辑（新文本坐标）；
//! 逆编辑只在事务进入历史时保留，避免 SkipHistory 大事务白存被删文本。
//!
//! 历史图只引用版本区间，不再复制 `EditList`。

use std::sync::Arc;

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

/// 单调版本索引的不可变编辑日志。
///
/// 每次提交生成包含新条目的新日志，旧 Snapshot 继续看到自己的版本区间；
/// 超出编辑历史预算的条目从最老端裁剪，无法再解析的 Anchor 由调用方回退。
#[derive(Debug, Clone, Default)]
pub(crate) struct EditLog {
    entries: Arc<[VersionedEdit]>,
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
        let mut entries = Vec::with_capacity(self.entries.len() + 1);
        entries.extend(self.entries.iter().cloned());
        entries.push(VersionedEdit {
            old_version,
            new_version,
            forward,
            undo,
        });
        Self {
            entries: Arc::from(entries),
        }
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
        let total = self.entries.len();
        let mut keep_from = total.saturating_sub(max_entries);

        if max_bytes != 0 {
            let mut bytes = 0usize;
            let mut start = total;
            while start > keep_from {
                let entry = &self.entries[start - 1];
                let entry_bytes = entry.forward.replacement_bytes()
                    + entry
                        .undo
                        .as_ref()
                        .map_or(0, |undo| undo.replacement_bytes());
                // 最新条目无条件保留，避免预算小于单事务时同步窗口为空。
                if start < total && bytes + entry_bytes > max_bytes {
                    break;
                }
                bytes += entry_bytes;
                start -= 1;
            }
            keep_from = keep_from.max(start);
        }

        if keep_from == 0 {
            return self.clone();
        }
        Self {
            entries: Arc::from(self.entries[keep_from..].to_vec()),
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
        let entries = self.entries_for_range(since, current)?;

        let mut patch = TextPatch::default();
        for entry in entries {
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
        let entries = self.entries_for_range(start, end)?;
        let mut batches = Vec::with_capacity(entries.len());
        for entry in entries.iter().rev() {
            let Some(undo) = &entry.undo else {
                return Err(TextError::InvariantViolation {
                    location: "EditLog::undo_batches",
                    detail: "历史节点引用了未保留逆编辑的版本".to_string(),
                });
            };
            batches.push((entry.old_version, entry.new_version, undo.clone()));
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
        let entries = self.entries_for_range(start, end)?;
        let mut batches = Vec::with_capacity(entries.len());
        for entry in entries.iter().rev() {
            let Some(undo) = &entry.undo else {
                return Err(TextError::HistoryTextUnavailable {
                    requested: start,
                    current: end,
                });
            };
            batches.push(undo.clone());
        }
        Ok(batches)
    }

    /// 取 `[start, end]` 版本区间的向前编辑与原始版本区间，按 redo 回放顺序（版本正序）返回。
    pub(crate) fn redo_batches(
        &self,
        start: BufferVersion,
        end: BufferVersion,
    ) -> TextResult<Vec<(BufferVersion, BufferVersion, EditList)>> {
        let entries = self.entries_for_range(start, end)?;
        Ok(entries
            .iter()
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
            .is_ok_and(|entries| entries.iter().all(|entry| entry.undo.is_some()))
    }

    /// 定位 `[start, end]` 的连续版本区间。
    fn entries_for_range(
        &self,
        start: BufferVersion,
        end: BufferVersion,
    ) -> TextResult<&[VersionedEdit]> {
        let Some(start_index) = self
            .entries
            .iter()
            .position(|entry| entry.old_version == start)
        else {
            return Err(self.evicted(start, end));
        };

        let mut index = start_index;
        let mut version = start;
        while index < self.entries.len() {
            let entry = &self.entries[index];
            if entry.old_version != version {
                break;
            }
            version = entry.new_version;
            index += 1;
            if version == end {
                break;
            }
        }
        if version != end {
            return Err(self.evicted(start, end));
        }
        Ok(&self.entries[start_index..index])
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
