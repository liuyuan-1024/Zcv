//! 差异协调器：打开文件差异的需求、版本与任务生命周期。
//!
//! 每个关注路径持有带 generation 的查询记录；文件内容、状态或 HEAD 变化时生成新版本，旧任务结果随后因版本不匹配被丢弃。
//! GitStore 只负责提供仓库事实（status 查询）和驱动后台执行（schedule_job）。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::Context;
use zcv_git::DiffHunk;
use zcv_git::FileStatus;

use super::{GitJob, GitStore, GitStoreEvent, canonicalize_path};

#[derive(Clone, Debug)]
pub(super) enum HunkState {
    /// 当前文件不需要查询差异，例如干净文件、未跟踪文件或仓库外文件。
    NotNeeded,
    /// 当前版本需要查询，但尚未进入任务队列。
    Unloaded,
    /// 已加入等待批次。
    Queued,
    /// 已交给后台线程处理。
    Loading,
    /// 当前版本的查询结果已经可用，空集合表示没有行级差异。
    Ready(Arc<[DiffHunk]>),
    /// 当前版本查询失败；只在文件再次变化后重试，避免失败自激循环。
    Failed(Arc<str>),
}

#[derive(Clone, Debug)]
pub(super) struct HunkRecord {
    pub(super) generation: u64,
    pub(super) state: HunkState,
}

/// 打开文件差异的需求、版本与任务生命周期。
pub(super) struct DiffCoordinator {
    pub(super) interests: BTreeSet<PathBuf>,
    pub(super) records: HashMap<PathBuf, HunkRecord>,
    pub(super) pending: BTreeMap<PathBuf, u64>,
    pub(super) in_flight: HashMap<PathBuf, u64>,
    pub(super) next_generation: u64,
}

impl DiffCoordinator {
    pub(super) fn new() -> Self {
        Self {
            interests: BTreeSet::new(),
            records: HashMap::new(),
            pending: BTreeMap::new(),
            in_flight: HashMap::new(),
            next_generation: 1,
        }
    }

    fn next_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        generation
    }

    /// 处理差异查询结果批次：按 generation 匹配丢弃过期结果；
    /// 批次内未出现的在途项（仓库在查询期间消失）进入失败终态。
    /// 返回是否有记录状态变化（供调用方决定是否发事件）。
    pub(super) fn complete_batch(
        &mut self,
        completed: impl IntoIterator<Item = (PathBuf, Result<Vec<DiffHunk>, String>)>,
    ) -> bool {
        let mut changed = false;
        for (path, result) in completed {
            let Some(dispatched_generation) = self.in_flight.remove(&path) else {
                continue;
            };
            let Some(record) = self.records.get_mut(&path) else {
                continue;
            };
            if record.generation != dispatched_generation
                || !matches!(record.state, HunkState::Loading)
            {
                continue;
            }
            record.state = match result {
                Ok(hunks) => HunkState::Ready(Arc::from(hunks)),
                Err(error) => HunkState::Failed(Arc::from(error)),
            };
            changed = true;
        }
        // 同一时刻只运行一个差异批次；仓库在任务期间消失时，无法映射的遗留项也必须进入失败终态。
        for (path, generation) in self.in_flight.drain() {
            if let Some(record) = self.records.get_mut(&path)
                && record.generation == generation
                && matches!(record.state, HunkState::Loading)
            {
                record.state = HunkState::Failed("仓库在差异查询期间已不可用".into());
                changed = true;
            }
        }
        changed
    }
}

impl GitStore {
    /// 用当前打开编辑器集合替换差异需求。
    /// 任务调度由状态机单向驱动，不依赖任务状态事件反向触发。
    pub fn set_hunk_interests(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let interests: BTreeSet<PathBuf> = paths
            .iter()
            .map(|path| canonicalize_path(path))
            .filter(|path| {
                self.root
                    .as_deref()
                    .is_some_and(|root| path.starts_with(root))
            })
            .collect();
        self.diff_coordinator.interests = interests;
        let interests = &self.diff_coordinator.interests;
        self.diff_coordinator
            .records
            .retain(|path, _| interests.contains(path));
        self.diff_coordinator
            .pending
            .retain(|path, _| interests.contains(path));
        self.ensure_interested_hunks(cx);
    }

    /// 增量增加差异需求，供无需维护完整打开文件集合的调用方使用。
    pub fn request_hunks(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = paths
            .iter()
            .map(|path| canonicalize_path(path))
            .filter(|path| {
                self.root
                    .as_deref()
                    .is_some_and(|root| path.starts_with(root))
            })
            .collect();
        if paths.is_empty() {
            return;
        }
        self.diff_coordinator.interests.extend(paths);
        self.ensure_interested_hunks(cx);
    }

    pub(super) fn path_needs_hunks(&self, path: &Path) -> bool {
        self.status_for_path(path)
            .is_some_and(|entry| matches!(entry.status, FileStatus::Tracked { .. }))
    }

    /// 把尚未建模的关注路径归类，并仅将 Unloaded 状态推进到 Queued。
    pub(super) fn ensure_interested_hunks(&mut self, cx: &mut Context<Self>) {
        let interests: Vec<PathBuf> = self.diff_coordinator.interests.iter().cloned().collect();
        for path in interests {
            let needs_hunks = self.path_needs_hunks(&path);
            if !self.diff_coordinator.records.contains_key(&path) {
                let generation = self.diff_coordinator.next_generation();
                self.diff_coordinator.records.insert(
                    path.clone(),
                    HunkRecord {
                        generation,
                        state: if needs_hunks {
                            HunkState::Unloaded
                        } else {
                            HunkState::NotNeeded
                        },
                    },
                );
            }
            let Some(record) = self.diff_coordinator.records.get_mut(&path) else {
                continue;
            };
            if needs_hunks && matches!(record.state, HunkState::Unloaded) {
                record.state = HunkState::Queued;
                self.diff_coordinator
                    .pending
                    .insert(path, record.generation);
            }
        }
        self.schedule_pending_hunks(cx);
    }

    pub(super) fn schedule_pending_hunks(&mut self, cx: &mut Context<Self>) {
        if !self.diff_coordinator.pending.is_empty() {
            self.schedule_job(GitJob::RefreshHunks, cx);
        }
    }

    /// 文件内容、状态或 HEAD 变化时为受影响的关注路径创建新版本；旧任务结果随后会因版本不匹配被丢弃。
    pub(super) fn invalidate_hunks_for_paths(
        &mut self,
        changed_paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        let affected: Vec<PathBuf> = self
            .diff_coordinator
            .interests
            .iter()
            .filter(|path| {
                changed_paths
                    .iter()
                    .any(|changed| path.starts_with(changed))
            })
            .cloned()
            .collect();
        if affected.is_empty() {
            return;
        }
        for path in &affected {
            let generation = self.diff_coordinator.next_generation();
            let state = if self.path_needs_hunks(path) {
                HunkState::Unloaded
            } else {
                HunkState::NotNeeded
            };
            self.diff_coordinator
                .records
                .insert(path.clone(), HunkRecord { generation, state });
            self.diff_coordinator.pending.remove(path);
        }
        self.ensure_interested_hunks(cx);
        cx.emit(GitStoreEvent::HunksChanged);
    }

    pub(super) fn invalidate_all_hunks(&mut self, cx: &mut Context<Self>) {
        let interests: Vec<PathBuf> = self.diff_coordinator.interests.iter().cloned().collect();
        self.invalidate_hunks_for_paths(&interests, cx);
    }
}
