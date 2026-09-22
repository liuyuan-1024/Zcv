//! 快照合并：后台任务结果回填仓库状态快照与 hunk 记录。
//!
//! commit_job 按 job 类型比对旧快照，仅在真实变化时发出事件；
//! 增量刷新经 merge_refresh 合并进旧快照，不整体重建。

use std::collections::BTreeSet;

use gpui::Context;
use zcv_git::GitRevision;
use zcv_path::AbsolutePathBuf;

use super::{
    GitStore, GitStoreEvent, Repository,
    background::{JobResult, merge_refresh},
    jobs::GitJob,
};

impl GitStore {
    pub(super) fn commit_job(&mut self, job: &GitJob, result: JobResult, cx: &mut Context<Self>) {
        match (job, result) {
            (GitJob::ReloadGitState, JobResult::Reload(scans)) => {
                let auto_resolve_paths = scans
                    .iter()
                    .flat_map(|scan| {
                        scan.clean_conflicts
                            .iter()
                            .map(|path| scan.working_directory.join_relative(path))
                    })
                    .collect::<Vec<_>>();
                let was_repository_scan_ready = self.repository_scan_ready;
                let old_work_dirs: BTreeSet<AbsolutePathBuf> = self
                    .repositories
                    .iter()
                    .map(|repository| {
                        super::repository_working_directory(repository.repository.as_ref())
                    })
                    .collect();
                let new_work_dirs: BTreeSet<AbsolutePathBuf> = scans
                    .iter()
                    .map(|scan| scan.working_directory.clone())
                    .collect();

                let mut head_changed = false;
                let mut statuses_changed = false;
                // 只有状态真正变化的路径才需要失效 index 文本；
                // 整体失效会让所有已加载文件重新读取并重挂 diff，产生一次全量投影抖动。
                let mut changed_index_paths = Vec::new();
                for scan in &scans {
                    let prev = self.repositories.iter().find(|repository| {
                        super::repository_working_directory(repository.repository.as_ref())
                            == scan.working_directory
                    });
                    head_changed |= prev.is_none_or(|prev| {
                        prev.snapshot.head != scan.snapshot.head
                            || prev.snapshot.branch != scan.snapshot.branch
                            || prev.snapshot.has_remote != scan.snapshot.has_remote
                            || prev.snapshot.ahead != scan.snapshot.ahead
                            || prev.snapshot.behind != scan.snapshot.behind
                    });
                    let Some(prev) = prev else {
                        // 新发现的仓库没有旧快照可比：该仓库当前所有路径都按变化处理。
                        statuses_changed = true;
                        changed_index_paths.extend(
                            scan.snapshot
                                .statuses_by_path
                                .keys()
                                .map(|path| scan.working_directory.join_relative(path)),
                        );
                        continue;
                    };
                    let previous = &prev.snapshot.statuses_by_path;
                    let current = &scan.snapshot.statuses_by_path;
                    statuses_changed |= previous != current;
                    for (path, entry) in current {
                        if previous.get(path) != Some(entry) {
                            changed_index_paths.push(scan.working_directory.join_relative(path));
                        }
                    }
                    for path in previous.keys() {
                        if !current.contains_key(path) {
                            changed_index_paths.push(scan.working_directory.join_relative(path));
                        }
                    }
                }

                if old_work_dirs != new_work_dirs {
                    cx.emit(GitStoreEvent::Repositories);
                }
                if head_changed {
                    // HEAD 变化：已加载/被引用的 HEAD 文本就地重读，新文本推送给共享 diff。
                    self.refresh_all_revision_documents(GitRevision::Head, cx);
                    cx.emit(GitStoreEvent::Head);
                }
                if statuses_changed {
                    // HEAD 变化（checkout/commit）会让 index 整体改写，且干净文件的 status 条目前后一致，无法用路径差识别，必须整体刷新。
                    if head_changed {
                        self.refresh_all_revision_documents(GitRevision::Index, cx);
                    } else {
                        self.refresh_revision_documents(
                            GitRevision::Index,
                            &changed_index_paths,
                            cx,
                        );
                    }
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
                if !auto_resolve_paths.is_empty() {
                    self.resolve_conflicts(auto_resolve_paths, cx);
                }
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
                            AbsolutePathBuf::canonicalize(repository.repository.working_directory())
                                .expect("Git 仓库工作目录必须是绝对路径")
                        })
                    });
                if self.active_repo_workdir != new_active {
                    self.active_repo_workdir = new_active;
                    cx.emit(GitStoreEvent::ActiveRepositoryChanged);
                }
            }
            (GitJob::RefreshStatuses, JobResult::Refresh(refreshed)) => {
                let auto_resolve_paths = refreshed
                    .iter()
                    .flat_map(|(index, data)| {
                        let workdir = self.repositories.get(*index).map(|repository| {
                            super::repository_working_directory(repository.repository.as_ref())
                        });
                        data.clean_conflicts.iter().filter_map(move |path| {
                            workdir.as_ref().map(|workdir| workdir.join_relative(path))
                        })
                    })
                    .collect::<Vec<_>>();
                let mut statuses_changed = false;
                let mut head_changed = false;
                let mut changed_paths = Vec::new();
                for (index, data) in refreshed {
                    let Some(repository) = self.repositories.get_mut(index) else {
                        continue;
                    };
                    let workdir =
                        super::repository_working_directory(repository.repository.as_ref());
                    changed_paths.extend(data.paths.iter().map(|path| workdir.join_relative(path)));
                    let (statuses, head) = merge_refresh(&mut repository.snapshot, data);
                    statuses_changed |= statuses;
                    head_changed |= head;
                }
                if !changed_paths.is_empty() {
                    self.refresh_revision_documents(GitRevision::Index, &changed_paths, cx);
                }
                if head_changed {
                    // HEAD 变化：已加载/被引用的 HEAD 文本就地重读。
                    self.refresh_all_revision_documents(GitRevision::Head, cx);
                    cx.emit(GitStoreEvent::Head);
                }
                // 先发布不可变索引，再发状态事件；订阅方收到事件时必须读取同一批刷新后的状态。
                self.rebuild_status_index();
                if !auto_resolve_paths.is_empty() {
                    self.resolve_conflicts(auto_resolve_paths, cx);
                }
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
            (GitJob::ApplyHunkEdits { diff, path, .. }, JobResult::GitOperation(result)) => {
                match result {
                    Ok(()) => {
                        // 乐观批次的基准由权威 index 修订文档安装新文本时清除；
                        // 这里只安排扫描，让权威文本决定 index 是否前进。
                        self.schedule_scan(cx);
                    }
                    Err(error) => {
                        // 真实写入失败：丢弃该路径的乐观批次并恢复显示层，再提示用户。
                        self.pending_index.remove(path);
                        diff.update(cx, |diff, cx| diff.clear_pending_hunks(cx));
                        cx.emit(GitStoreEvent::HunkOperationFailed(format!("{error:#}")));
                        self.schedule_scan(cx);
                    }
                }
            }
            (
                GitJob::GitInit
                | GitJob::StageFiles { .. }
                | GitJob::ResolveConflicts { .. }
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
                Err(error) => {
                    cx.emit(GitStoreEvent::UncommitFailed(format!("{error:#}")));
                }
            },
            _ => {}
        }
    }
}
