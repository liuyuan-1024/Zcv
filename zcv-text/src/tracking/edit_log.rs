//! 版本索引的向前编辑日志：Buffer 拥有的版本化编辑事实。
//!
//! 让 Snapshot 能重建“自某版本以来的净编辑”，供组合文档按 excerpt 锚点范围增量同步，并让 Anchor 无需携带 PositionMap 即可跨版本解析。

use std::sync::Arc;

use crate::{
    BufferVersion, TextError, TextRange, TextResult,
    text_changes::{TextChangeBatch, TextPatch},
};

/// 一次已提交版本推进的净编辑。
///
/// 只保留坐标事实（old/new 区间），不保存 replacement 文本；
/// replacement 仍由历史节点与源文本快照权威持有。
#[derive(Debug, Clone)]
struct VersionedEdit {
    old_version: BufferVersion,
    new_version: BufferVersion,
    patch: TextPatch,
    reset: bool,
}

/// 单调版本索引的不可变编辑日志。
///
/// 每次提交生成包含新条目的新日志，旧 Snapshot 继续看到自己的版本区间；
/// 超出 Undo 历史预算的条目从最老端裁剪，无法再解析的 Anchor 由调用方回退。
#[derive(Debug, Clone, Default)]
pub(crate) struct EditLog {
    entries: Arc<[VersionedEdit]>,
}

impl EditLog {
    /// 追加一次版本推进，并按 Undo 历史预算裁剪最老条目。
    pub(crate) fn appended(
        &self,
        old_version: BufferVersion,
        new_version: BufferVersion,
        patch: TextPatch,
        reset: bool,
        max_entries: usize,
    ) -> Self {
        let first = if max_entries != 0 && self.entries.len() >= max_entries {
            self.entries.len() + 1 - max_entries
        } else {
            0
        };
        let mut entries = Vec::with_capacity(self.entries.len() - first + 1);
        entries.extend(self.entries[first..].iter().cloned());
        entries.push(VersionedEdit {
            old_version,
            new_version,
            patch,
            reset,
        });
        Self {
            entries: Arc::from(entries),
        }
    }

    /// 组合 since 到 current 之间的连续编辑为单个批次。
    ///
    /// since == current 返回空批次；
    /// since 已被裁剪或不是本日志中的版本时返回 TextError::VersionEvicted，调用方必须回退而不是猜测坐标。
    pub(crate) fn batch_since(
        &self,
        since: BufferVersion,
        current: BufferVersion,
    ) -> TextResult<TextChangeBatch> {
        if since == current {
            return Ok(TextChangeBatch::default());
        }
        let start = self
            .entries
            .iter()
            .position(|entry| entry.old_version == since);
        let Some(start) = start else {
            return Err(self.evicted(since, current));
        };

        let mut patch = TextPatch::default();
        let mut reset = false;
        let mut version = since;
        for entry in &self.entries[start..] {
            if entry.old_version != version {
                return Err(self.evicted(since, current));
            }
            patch = patch.compose(&entry.patch);
            reset |= entry.reset;
            version = entry.new_version;
        }
        if version != current {
            return Err(self.evicted(since, current));
        }
        Ok(TextChangeBatch::from_patch(since, current, patch, reset))
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
