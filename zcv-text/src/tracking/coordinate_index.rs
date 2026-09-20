//! 版本 → 坐标增量的不衰减索引。
//!
//! 每个已提交版本推进追加一条只含坐标的增量（旧区间 + 新区间长度），
//! 不复制任何替换文本。与带文本的 EditLog 平行：EditLog 受
//! `max_edit_history_entries` / `max_edit_history_bytes` 从最老端裁剪，
//! 本索引永不裁剪，保证任意仍存在的旧版本都能把坐标映射到当前版本。
//!
//! reset / 基线替换会开启新代际；旧代际锚点由 `Anchor` 显式判定失效，
//! 不使用本索引继续映射。

use std::sync::Arc;

use crate::{text_changes::TextPatch, types::BufferVersion};

/// 单个块的坐标增量数。
///
/// 块式持久结构把“追加一次复制整条索引”降为“只复制当前块与块指针数组”，
/// 使不衰减索引的追加代价远低于每提交 O(版本数)。
const CHUNK: usize = 128;

/// 一次版本推进的纯坐标增量。
#[derive(Debug, Clone)]
struct CoordinateStep {
    old_version: BufferVersion,
    new_version: BufferVersion,
    /// 只保存 old range 与 new range（含 replacement 长度），不含替换文本。
    patch: TextPatch,
}

/// 版本到坐标增量的不衰减索引。
#[derive(Debug, Clone)]
pub(crate) struct CoordinateIndex {
    /// 已追加的块；除最后一块外都恰好 `CHUNK` 条。
    chunks: Arc<[Arc<[CoordinateStep]>]>,
    /// 已索引的步数。
    len: usize,
    /// 最早被索引的版本；`len == 0` 时无意义。
    first_version: BufferVersion,
}

impl Default for CoordinateIndex {
    fn default() -> Self {
        Self {
            chunks: Arc::from(Vec::new()),
            len: 0,
            first_version: BufferVersion::INITIAL,
        }
    }
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
        let step = CoordinateStep {
            old_version,
            new_version,
            patch,
        };
        let first_version = if self.len == 0 {
            old_version
        } else {
            self.first_version
        };

        let mut chunks: Vec<Arc<[CoordinateStep]>> = self.chunks.to_vec();
        match chunks.last_mut() {
            Some(tail) if tail.len() < CHUNK => {
                let mut grown = Vec::with_capacity(tail.len() + 1);
                grown.extend_from_slice(tail);
                grown.push(step);
                *tail = Arc::from(grown);
            }
            _ => chunks.push(Arc::from([step])),
        }

        Self {
            chunks: Arc::from(chunks),
            len: self.len + 1,
            first_version,
        }
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
        if self.len == 0 || since < self.first_version {
            return None;
        }
        let start = (since.get() - self.first_version.get()) as usize;
        if start >= self.len {
            return None;
        }

        let mut patch = TextPatch::default();
        let mut version = since;
        for index in start..self.len {
            let step = self.step_at(index);
            if step.old_version != version {
                return None;
            }
            patch = patch.compose(&step.patch);
            version = step.new_version;
            if version == current {
                return Some(patch);
            }
        }
        None
    }

    fn step_at(&self, index: usize) -> &CoordinateStep {
        let (chunk, offset) = (index / CHUNK, index % CHUNK);
        &self.chunks[chunk][offset]
    }
}

#[cfg(test)]
#[path = "test/coordinate_index_tests.rs"]
mod tests;
