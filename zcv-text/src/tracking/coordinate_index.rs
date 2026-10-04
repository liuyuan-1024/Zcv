//! 版本 → 坐标增量的不衰减索引。
//!
//! 每个已提交版本推进追加一条只含坐标的增量（旧区间 + 新区间长度），
//! 不复制任何替换文本。与带文本的 EditLog 平行：EditLog 受
//! `max_edit_history_entries` / `max_edit_history_bytes` 从最老端裁剪，
//! 本索引永不裁剪，保证任意仍存在的旧版本都能把坐标映射到当前版本。
//!
//! 外部文本更新也作为普通版本推进追加到本索引，所有锚点统一沿同一坐标链映射。

use sum_tree::{Bias, ContextLessSummary, Item, SumTree};

use crate::{text_changes::TextPatch, types::BufferVersion};

/// 一次版本推进的纯坐标增量。
#[derive(Debug, Clone)]
struct CoordinateStep {
    old_version: BufferVersion,
    new_version: BufferVersion,
    /// 只保存 old range 与 new range（含 replacement 长度），不含替换文本。
    patch: TextPatch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct StepCount(usize);

impl ContextLessSummary for StepCount {
    fn zero() -> Self {
        Self(0)
    }

    fn add_summary(&mut self, other: &Self) {
        self.0 += other.0;
    }
}

impl Item for CoordinateStep {
    type Summary = StepCount;

    fn summary(&self, (): ()) -> Self::Summary {
        StepCount(1)
    }
}

/// 版本到坐标增量的不衰减索引；追加时共享既有树节点。
#[derive(Debug, Clone, Default)]
pub(crate) struct CoordinateIndex {
    steps: SumTree<CoordinateStep>,
}

impl CoordinateIndex {
    /// 追加一次版本推进的坐标增量。
    ///
    /// 调用方保证 `old_version` 与本索引最新版本连续。
    pub(crate) fn appended(
        &self,
        old_version: BufferVersion,
        new_version: BufferVersion,
        patch: TextPatch,
    ) -> Self {
        let mut next = self.clone();
        if let Some(last) = next.steps.last() {
            assert_eq!(last.new_version, old_version, "坐标索引版本必须连续");
        }
        assert_eq!(old_version.next(), Some(new_version), "版本必须推进一步");
        next.steps.push(
            CoordinateStep {
                old_version,
                new_version,
                patch,
            },
            (),
        );
        next
    }

    /// 组合 `since` 到 `current` 的连续坐标增量。
    ///
    /// `since == current` 返回空 Patch；`since` 不在本索引覆盖范围内时返回 None。
    pub(crate) fn patch_since(
        &self,
        since: BufferVersion,
        current: BufferVersion,
    ) -> Option<TextPatch> {
        if since == current {
            return Some(TextPatch::default());
        }
        let first_version = self.steps.first()?.old_version;
        if since < first_version {
            return None;
        }
        let start = usize::try_from(since.get() - first_version.get()).ok()?;
        if start >= self.steps.summary().0 {
            return None;
        }

        let mut patch = TextPatch::default();
        let mut version = since;
        let mut cursor = self.steps.cursor::<StepCount>(());
        cursor.seek(&StepCount(start), Bias::Right);
        while let Some(step) = cursor.item() {
            if step.old_version != version {
                return None;
            }
            patch = patch.compose(&step.patch);
            version = step.new_version;
            if version == current {
                return Some(patch);
            }
            cursor.next();
        }
        None
    }
}

#[cfg(test)]
#[path = "test/coordinate_index_tests.rs"]
mod tests;
