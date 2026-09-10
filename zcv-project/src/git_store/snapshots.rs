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
                let was_repository_scan_ready = self.repository_scan_ready;
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
                    self.invalidate_revision_text(zcv_git::GitRevision::Head);
                    cx.emit(GitStoreEvent::Head);
                }
                if statuses_changed {
                    self.invalidate_revision_text(zcv_git::GitRevision::Index);
                    cx.emit(GitStoreEvent::Statuses);
                }
                self.repositories = scans
                    .into_iter()
                    .map(|scan| Repository {
                        repository: scan.repository,
                        snapshot: scan.snapshot,
                    })
                    .collect();
                self.repository_scan_ready = true;
                if !was_repository_scan_ready {
                    cx.emit(GitStoreEvent::Repositories);
                }
                self.rebuild_status_index();
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
                if !changed_paths.is_empty() {
                    self.invalidate_revision_text_for_paths(
                        zcv_git::GitRevision::Index,
                        &changed_paths,
                    );
                }
                if head_changed {
                    // HEAD 变化 → 旧 HEAD 文本失效。
                    self.invalidate_revision_text(zcv_git::GitRevision::Head);
                    cx.emit(GitStoreEvent::Head);
                }
                // 先发布不可变索引，再发状态事件；订阅方收到事件时必须读取同一批刷新后的状态。
                self.rebuild_status_index();
                if statuses_changed {
                    cx.emit(GitStoreEvent::Statuses);
                }
            }
            (GitJob::GitOperation { .. }, JobResult::GitOperation(result)) => {
                // 操作改变了引用/工作树：重新全量扫描，比对后发出 Repositories/Head/Statuses 事件。
                if result.is_ok() {
                    self.schedule_scan(cx);
                }
            }
            (GitJob::ApplyHunkEdits { diff, .. }, JobResult::GitOperation(result)) => {
                match result {
                    Ok(()) => {
                        // 成功：保留 optimistic 状态直到后续权威扫描替换该 diff，避免中途回闪。
                        self.schedule_scan(cx);
                    }
                    Err(error) => {
                        // 失败：清除 optimistic 状态并通知显示层恢复真实 diff。
                        diff.update(cx, |diff, cx| diff.clear_pending_hunks(cx));
                        cx.emit(GitStoreEvent::HunkOperationFailed(format!("{error:#}")));
                    }
                }
            }
            (
                GitJob::GitInit
                | GitJob::StageFiles { .. }
                | GitJob::Commit { .. }
                | GitJob::CheckoutBranch { .. }
                | GitJob::CreateBranch { .. }
                | GitJob::DeleteBranch { .. },
                JobResult::GitOperation(result),
            ) => {
                if let Ok(()) = result {
                    // 操作改变了引用/工作树：重新全量扫描，比对后发出 Repositories/Head/Statuses 事件。
                    self.schedule_scan(cx);
                }
            }
            (GitJob::Uncommit, JobResult::Uncommit(result)) => match result {
                Ok(Some(message)) => {
                    // 事件直接携带被撤销消息，面板订阅后填回提交信息编辑器，无需跨事件暂存。
                    cx.emit(GitStoreEvent::Uncommitted(message));
                    self.schedule_scan(cx);
                }
                Ok(None) => {
                    self.schedule_scan(cx);
                }
                Err(_) => {}
            },
            _ => {}
        }
    }
}
