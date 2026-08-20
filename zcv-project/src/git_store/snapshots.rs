//! 快照合并：后台任务结果回填仓库状态快照与 hunk 记录。
//!
//! commit_job 按 job 类型比对旧快照，仅在真实变化时发出事件；
//! 增量刷新经 merge_refresh 合并进旧快照，不整体重建。

use std::collections::BTreeSet;
use std::path::PathBuf;

use gpui::Context;

use super::{
    GitStore, GitStoreEvent, Repository,
    background::{JobResult, merge_refresh},
    jobs::GitJob,
};

impl GitStore {
    pub(super) fn commit_job(&mut self, job: &GitJob, result: JobResult, cx: &mut Context<Self>) {
        match (job, result) {
            (GitJob::ReloadGitState, JobResult::Reload(scans)) => {
                let old_work_dirs: BTreeSet<PathBuf> = self
                    .repositories
                    .iter()
                    .map(|repository| repository.repository.working_directory().to_path_buf())
                    .collect();
                let new_work_dirs: BTreeSet<PathBuf> = scans
                    .iter()
                    .map(|scan| scan.working_directory.clone())
                    .collect();

                let mut head_changed = false;
                let mut statuses_changed = false;
                for scan in &scans {
                    let prev = self.repositories.iter().find(|repository| {
                        repository.repository.working_directory() == scan.working_directory
                    });
                    head_changed |= prev.is_none_or(|prev| {
                        prev.snapshot.head != scan.snapshot.head
                            || prev.snapshot.branch != scan.snapshot.branch
                            || prev.snapshot.has_remote != scan.snapshot.has_remote
                            || prev.snapshot.ahead != scan.snapshot.ahead
                            || prev.snapshot.behind != scan.snapshot.behind
                    });
                    statuses_changed |= prev.is_none_or(|prev| {
                        prev.snapshot.statuses_by_path != scan.snapshot.statuses_by_path
                    });
                }

                if old_work_dirs != new_work_dirs {
                    cx.emit(GitStoreEvent::Repositories);
                }
                if head_changed {
                    // HEAD 变化 → 旧 HEAD 文本失效。
                    self.committed_text_cache.clear();
                    cx.emit(GitStoreEvent::Head);
                }
                if statuses_changed {
                    cx.emit(GitStoreEvent::Statuses);
                }
                self.repositories = scans
                    .into_iter()
                    .map(|scan| Repository {
                        repository: scan.repository,
                        snapshot: scan.snapshot,
                    })
                    .collect();
                // 活动仓库维护：仍在集合中则保持；否则回退新集合第一个（Vec 序 = 祖先在前，与默认候选一致）；
                // 集合为空 → None。注意用 repositories 而非 new_work_dirs：BTreeSet 按字典序迭代，取不到发现顺序。
                // emit 是 deferred（pending_effects），订阅方永远读到赋值后的完整状态，首次扫描 None → Some(第一个) 恰好触发一次。
                let new_active = self
                    .active_repo_workdir
                    .as_ref()
                    .filter(|workdir| new_work_dirs.contains(*workdir))
                    .cloned()
                    .or_else(|| {
                        self.repositories.first().map(|repository| {
                            repository.repository.working_directory().to_path_buf()
                        })
                    });
                if self.active_repo_workdir != new_active {
                    self.active_repo_workdir = new_active;
                    cx.emit(GitStoreEvent::ActiveRepositoryChanged);
                }
                if head_changed || statuses_changed || old_work_dirs != new_work_dirs {
                    self.invalidate_all_hunks(cx);
                }
                log::info!("git 状态已刷新：{} 个仓库", self.repositories.len());
            }
            (GitJob::RefreshStatuses, JobResult::Refresh(refreshed)) => {
                let mut statuses_changed = false;
                let mut head_changed = false;
                let mut changed_paths = Vec::new();
                for (index, data) in refreshed {
                    let Some(repository) = self.repositories.get_mut(index) else {
                        continue;
                    };
                    let workdir = repository.repository.working_directory().to_path_buf();
                    changed_paths.extend(data.paths.iter().map(|path| workdir.join(path)));
                    let (statuses, head) = merge_refresh(&mut repository.snapshot, data);
                    statuses_changed |= statuses;
                    head_changed |= head;
                }
                if head_changed {
                    // HEAD 变化 → 旧 HEAD 文本失效。
                    self.committed_text_cache.clear();
                    cx.emit(GitStoreEvent::Head);
                }
                if statuses_changed {
                    cx.emit(GitStoreEvent::Statuses);
                }
                if head_changed {
                    self.invalidate_all_hunks(cx);
                } else {
                    // 即使状态枚举与行数统计没有变化，文件内容也可能已经改变，因此刷新路径总会产生新差异版本。
                    self.invalidate_hunks_for_paths(&changed_paths, cx);
                }
            }
            (GitJob::RefreshHunks, JobResult::RefreshHunks(refreshed)) => {
                let mut completed = Vec::new();
                for (index, results) in refreshed {
                    let Some(workdir) = self
                        .repositories
                        .get(index)
                        .map(|repository| repository.repository.working_directory().to_path_buf())
                    else {
                        continue;
                    };
                    completed.extend(
                        results
                            .into_iter()
                            .map(|(path, result)| (workdir.join(path), result)),
                    );
                }
                if self.diff_coordinator.complete_batch(completed) {
                    cx.emit(GitStoreEvent::HunksChanged);
                }
            }
            (GitJob::GitOperation { operation, .. }, JobResult::GitOperation(result)) => {
                match &result {
                    Ok(()) => {
                        log::info!("git {operation:?} 成功");
                    }
                    Err(error) => {
                        log::warn!("git {operation:?} 失败：{error:#}");
                    }
                }
                // 操作改变了引用/工作树：重新全量扫描，比对后发出 Repositories/Head/Statuses 事件。
                if result.is_ok() {
                    self.schedule_scan(cx);
                }
            }
            (
                job @ (GitJob::GitInit
                | GitJob::StageFiles { .. }
                | GitJob::Commit { .. }
                | GitJob::CheckoutBranch { .. }
                | GitJob::CreateBranch { .. }),
                JobResult::GitOperation(result),
            ) => {
                match result {
                    Ok(()) => {
                        // 操作改变了引用/工作树：重新全量扫描，比对后发出 Repositories/Head/Statuses 事件。
                        log::info!("git {job:?} 成功");
                        self.schedule_scan(cx);
                    }
                    Err(error) => {
                        log::warn!("git {job:?} 失败：{error:#}");
                    }
                }
            }
            (GitJob::Uncommit, JobResult::Uncommit(result)) => match result {
                Ok(message) => {
                    // 被撤销消息暂存，Head 事件后由面板读取填回提交信息编辑器。
                    self.pending_uncommitted_message = message;
                    log::info!("git uncommit 成功");
                    self.schedule_scan(cx);
                }
                Err(error) => {
                    log::warn!("git uncommit 失败：{error:#}");
                }
            },
            _ => {}
        }
    }
}
