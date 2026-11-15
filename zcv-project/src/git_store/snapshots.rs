//! 快照合并：后台任务结果回填仓库状态快照与 hunk 记录。
//!
//! commit_job 按 job 类型比对旧快照，仅在真实变化时发出事件；
//! 增量刷新经 merge_refresh 合并进旧快照，不整体重建。

use std::{collections::HashMap, sync::Arc};

use gpui::Context;
use zcv_git::{GitRepository, GitRevision};

use super::{
    GitScanState, GitStore, GitStoreEvent, Repository,
    background::{JobResult, merge_refresh},
    jobs::GitJob,
    repository_working_directory,
};

impl GitStore {
    pub(super) fn apply_repository_discovery(
        &mut self,
        result: anyhow::Result<Vec<Arc<dyn GitRepository>>>,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Arc<dyn GitRepository>>> {
        let discovered = match result {
            Ok(discovered) => discovered,
            Err(error) => {
                let message = format!("发现 Git 仓库失败：{error:#}");
                let state = GitScanState::Failed(message.clone());
                if self.repository_discovery != state {
                    self.repository_discovery = state;
                    cx.emit(GitStoreEvent::RepositoryDiscoveryChanged);
                    cx.emit(GitStoreEvent::StatusScanFailed(message));
                }
                return None;
            }
        };

        let old_paths: Vec<_> = self
            .repositories
            .iter()
            .map(|repository| repository_working_directory(repository.repository.as_ref()))
            .collect();
        let new_paths: Vec<_> = discovered
            .iter()
            .map(|repository| repository_working_directory(repository.as_ref()))
            .collect();
        let repositories_changed = old_paths != new_paths;
        if repositories_changed {
            let mut previous: HashMap<_, _> = old_paths
                .into_iter()
                .zip(std::mem::take(&mut self.repositories))
                .collect();
            self.repositories = new_paths
                .iter()
                .zip(&discovered)
                .map(|(path, repository)| {
                    previous.remove(path).unwrap_or_else(|| Repository {
                        repository: repository.clone(),
                        status: GitScanState::Pending,
                    })
                })
                .collect();
            self.rebuild_status_index();
            cx.emit(GitStoreEvent::Repositories);
        }
        if self.repository_discovery != GitScanState::Ready(()) {
            self.repository_discovery = GitScanState::Ready(());
            cx.emit(GitStoreEvent::RepositoryDiscoveryChanged);
        }

        if self
            .active_repo_workdir
            .as_ref()
            .is_none_or(|active| !new_paths.contains(active))
        {
            let active = new_paths.into_iter().next();
            if self.active_repo_workdir != active {
                self.active_repo_workdir = active;
                cx.emit(GitStoreEvent::ActiveRepositoryChanged);
            }
        }

        Some(
            self.repositories
                .iter()
                .map(|repository| repository.repository.clone())
                .collect(),
        )
    }

    pub(super) fn commit_job(&mut self, job: &GitJob, result: JobResult, cx: &mut Context<Self>) {
        match (job, result) {
            (GitJob::ReloadGitState, JobResult::Reload(results)) => {
                let mut head_changed = false;
                let mut statuses_changed = false;
                let mut first_error = None;
                let mut auto_resolve_paths = Vec::new();
                let mut repositories: HashMap<_, _> = self
                    .repositories
                    .iter_mut()
                    .map(|repository| {
                        (
                            repository_working_directory(repository.repository.as_ref()),
                            repository,
                        )
                    })
                    .collect();
                for result in results {
                    let path = match &result {
                        Ok(scan) => &scan.working_directory,
                        Err((path, _)) => path,
                    };
                    let Some(repository) = repositories.remove(path) else {
                        continue;
                    };
                    match result {
                        Ok(scan) => {
                            let current = &scan.snapshot;
                            if let Some(previous) = repository.status.ready() {
                                head_changed |= previous.head != current.head
                                    || previous.branch != current.branch
                                    || previous.has_remote != current.has_remote
                                    || previous.ahead != current.ahead
                                    || previous.behind != current.behind;
                                statuses_changed |=
                                    previous.statuses_by_path != current.statuses_by_path;
                            } else {
                                head_changed |= current.head.is_some() || current.branch.is_some();
                                statuses_changed = true;
                            }
                            repository.status = GitScanState::Ready(scan.snapshot);
                            auto_resolve_paths.extend(
                                scan.clean_conflicts
                                    .iter()
                                    .map(|path| scan.working_directory.join_relative(path)),
                            );
                        }
                        Err((path, error)) => {
                            let message = format!(
                                "扫描 Git 状态失败（{}）：{error}",
                                path.as_path().display()
                            );
                            match &repository.status {
                                GitScanState::Ready(_) => {}
                                GitScanState::Failed(previous) if *previous == message => {}
                                GitScanState::Pending | GitScanState::Failed(_) => {
                                    repository.status = GitScanState::Failed(message.clone());
                                    statuses_changed = true;
                                }
                            }
                            first_error.get_or_insert(message);
                        }
                    }
                }
                self.report_scan_error(first_error, cx);
                if head_changed {
                    self.refresh_all_revision_documents(GitRevision::Head, cx);
                    cx.emit(GitStoreEvent::Head);
                }
                // 状态枚举和行数不能标识 index 内容；扫描确认所有已消费修订，文本未变时保持投影。
                self.refresh_all_revision_documents(GitRevision::Index, cx);
                if statuses_changed {
                    self.rebuild_status_index();
                    cx.emit(GitStoreEvent::Statuses);
                }
                if !auto_resolve_paths.is_empty() {
                    self.resolve_conflicts(auto_resolve_paths, cx);
                }
            }
            (GitJob::RefreshStatuses, JobResult::Refresh(refreshed)) => {
                let error = refreshed.iter().find_map(|(index, data)| {
                    data.as_ref().err().map(|error| {
                        let path = self
                            .repositories
                            .get(*index)
                            .map(|repository| {
                                repository
                                    .repository
                                    .working_directory()
                                    .display()
                                    .to_string()
                            })
                            .unwrap_or_default();
                        format!("刷新 Git 状态失败（{path}）：{error}")
                    })
                });
                self.report_scan_error(error, cx);
                let refreshed = refreshed
                    .into_iter()
                    .filter_map(|(index, data)| data.ok().map(|data| (index, data)))
                    .collect::<Vec<_>>();
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
                    let GitScanState::Ready(snapshot) = &mut repository.status else {
                        continue;
                    };
                    let (statuses, head) = merge_refresh(snapshot, data);
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
            (GitJob::Commit { workdir, .. }, JobResult::GitOperation(result)) => match result {
                Ok(()) => {
                    cx.emit(GitStoreEvent::Committed(workdir.clone()));
                    self.schedule_scan(cx);
                }
                Err(error) => cx.emit(GitStoreEvent::CommitFailed(
                    workdir.clone(),
                    format!("{error:#}"),
                )),
            },
            (GitJob::Uncommit { workdir }, JobResult::Uncommit(result)) => match result {
                Ok(Some(message)) => {
                    // 事件直接携带被撤销消息，面板订阅后填回提交信息编辑器，无需跨事件暂存。
                    cx.emit(GitStoreEvent::Uncommitted(workdir.clone(), message));
                    self.schedule_scan(cx);
                }
                Ok(None) => {
                    self.schedule_scan(cx);
                }
                Err(error) => {
                    cx.emit(GitStoreEvent::UncommitFailed(
                        workdir.clone(),
                        format!("{error:#}"),
                    ));
                }
            },
            _ => {}
        }
    }

    fn report_scan_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        if error != self.last_scan_error {
            if let Some(message) = &error {
                cx.emit(GitStoreEvent::StatusScanFailed(message.clone()));
            }
            self.last_scan_error = error;
        }
    }
}
