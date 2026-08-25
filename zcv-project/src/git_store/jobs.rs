//! 任务队列：job 类型族与排队/去重/阶段状态机。
//!
//! GitJob 按 key 去重（同 key 排队中/执行中时丢弃新 job），经 channel 交后台执行；
//! 阶段状态（Queued/Running/Cancelling/Reconciling）与在途标记在此维护。

use std::path::PathBuf;
use std::sync::Arc;

use gpui::Context;
use zcv_git::GitCancellation;

use super::{GitOperationOutcome, GitStore, GitStoreEvent};

/// job 标识。
pub(super) type GitJobId = u64;

/// job 执行阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitJobPhase {
    Queued,
    Running,
    Cancelling,
    Reconciling,
}

#[derive(Clone)]
pub struct GitJobStatus {
    pub name: Arc<str>,
    pub operation: Option<GitOperationKind>,
    pub phase: GitJobPhase,
    pub cancellable: bool,
    progress_source: Option<GitCancellation>,
}

impl GitJobStatus {
    pub fn progress(&self) -> Option<String> {
        self.progress_source
            .as_ref()
            .and_then(GitCancellation::progress)
    }
}

/// 用户触发的 git 操作（fetch/pull/push，由 UI 发起，后台执行）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GitOperationKind {
    Fetch,
    Pull,
    Push,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) enum GitJobKey {
    ReloadGitState,
    RefreshStatuses,
    RefreshHunks,
    GitOperation(GitOperationKind),
    GitInit,
    /// 暂存/取消暂存（路径集合参与 key：不同路径集互不合并，同路径重复点击在队列中自动去重，避免一次操作被意外丢弃）。
    StageFiles {
        stage: bool,
        paths: Vec<PathBuf>,
    },
    /// 提交（消息参与 key：同消息双击去重，改消息重试不被去重跳过）。
    Commit {
        message: String,
    },
    /// 撤销最近一次提交（无参：同一时间只允许一个 uncommit 在途）。
    Uncommit,
    /// 切换分支（名字参与 key：同名双击去重，改目标不被去重跳过）。
    CheckoutBranch {
        name: String,
    },
    /// 创建并切换分支（同上）。
    CreateBranch {
        name: String,
    },
}

#[derive(Clone, Debug)]
pub(super) enum GitJob {
    ReloadGitState,
    RefreshStatuses,
    RefreshHunks,
    GitOperation {
        operation: GitOperationKind,
        /// 操作结果回传通道（发起方 await 后弹提示）；内部调度时为 None。
        on_done: Option<async_channel::Sender<GitOperationOutcome>>,
    },
    GitInit,
    StageFiles {
        stage: bool,
        paths: Vec<PathBuf>,
    },
    Commit {
        message: String,
    },
    Uncommit,
    CheckoutBranch {
        name: String,
    },
    CreateBranch {
        name: String,
    },
}

#[derive(Clone)]
pub(super) struct ScheduledGitJob {
    pub(super) id: GitJobId,
    pub(super) job: GitJob,
    pub(super) cancellation: Option<GitCancellation>,
}

pub(super) struct GitJobRecord {
    pub(super) id: GitJobId,
    pub(super) key: GitJobKey,
    pub(super) name: Arc<str>,
    pub(super) operation: Option<GitOperationKind>,
    pub(super) phase: GitJobPhase,
    pub(super) cancellation: Option<GitCancellation>,
}

impl GitJob {
    fn key(&self) -> GitJobKey {
        match self {
            GitJob::ReloadGitState => GitJobKey::ReloadGitState,
            GitJob::RefreshStatuses => GitJobKey::RefreshStatuses,
            GitJob::RefreshHunks => GitJobKey::RefreshHunks,
            GitJob::GitOperation { operation, .. } => GitJobKey::GitOperation(*operation),
            GitJob::GitInit => GitJobKey::GitInit,
            GitJob::StageFiles { stage, paths } => GitJobKey::StageFiles {
                stage: *stage,
                paths: paths.clone(),
            },
            GitJob::Commit { message } => GitJobKey::Commit {
                message: message.clone(),
            },
            GitJob::Uncommit => GitJobKey::Uncommit,
            GitJob::CheckoutBranch { name } => GitJobKey::CheckoutBranch { name: name.clone() },
            GitJob::CreateBranch { name } => GitJobKey::CreateBranch { name: name.clone() },
        }
    }

    /// 状态栏展示用的任务名。
    fn task_name(&self) -> Arc<str> {
        match self {
            GitJob::ReloadGitState => "扫描仓库状态".into(),
            GitJob::RefreshStatuses => "刷新仓库状态".into(),
            GitJob::RefreshHunks => "查询文件差异".into(),
            GitJob::GitOperation {
                operation: GitOperationKind::Fetch,
                ..
            } => "拉取".into(),
            GitJob::GitOperation {
                operation: GitOperationKind::Pull,
                ..
            } => "合并拉取".into(),
            GitJob::GitOperation {
                operation: GitOperationKind::Push,
                ..
            } => "推送".into(),
            GitJob::GitInit => "初始化仓库".into(),
            GitJob::StageFiles { stage: true, .. } => "暂存".into(),
            GitJob::StageFiles { stage: false, .. } => "取消暂存".into(),
            GitJob::Commit { .. } => "提交".into(),
            GitJob::Uncommit => "撤销提交".into(),
            GitJob::CheckoutBranch { .. } => "切换分支".into(),
            GitJob::CreateBranch { .. } => "创建分支".into(),
        }
    }
}

impl GitStore {
    pub(super) fn schedule_job(&mut self, job: GitJob, cx: &mut Context<Self>) -> Option<GitJobId> {
        // 同 key 的 job 已在队列/执行中时，丢弃新 job（路径已累积在paths_needing_status_update，由正在执行的 job 统一消费）。
        let key = job.key();
        if self.pending_jobs.contains_key(&key) {
            return None;
        }
        let id = self.next_job_id;
        self.next_job_id = self.next_job_id.wrapping_add(1).max(1);
        let operation = match &job {
            GitJob::GitOperation { operation, .. } => Some(*operation),
            _ => None,
        };
        let cancellation = operation.map(|_| GitCancellation::new());
        let name = job.task_name();
        self.pending_jobs.insert(key.clone(), id);
        self.jobs.insert(
            id,
            GitJobRecord {
                id,
                key: key.clone(),
                name,
                operation,
                phase: GitJobPhase::Queued,
                cancellation: cancellation.clone(),
            },
        );
        if self
            .job_sender
            .try_send(ScheduledGitJob {
                id,
                job,
                cancellation,
            })
            .is_err()
        {
            self.finish_job(id, cx);
            return None;
        }
        cx.emit(GitStoreEvent::JobsUpdated);
        Some(id)
    }

    /// 当前可见任务。远程操作（含排队、取消、确认阶段）优先，确保用户点击后立即获得反馈；
    /// 后台自动扫描/刷新静默执行（指示器只展示用户可见操作）。
    pub fn current_job(&self) -> Option<GitJobStatus> {
        let record = self
            .jobs
            .values()
            .filter(|job| job.operation.is_some())
            .min_by_key(|job| job.id)
            .or_else(|| {
                self.in_flight
                    .and_then(|id| self.jobs.get(&id))
                    .filter(|job| {
                        !matches!(
                            job.key,
                            GitJobKey::ReloadGitState
                                | GitJobKey::RefreshStatuses
                                | GitJobKey::RefreshHunks
                        )
                    })
            })?;
        Some(GitJobStatus {
            name: record.name.clone(),
            operation: record.operation,
            phase: record.phase,
            cancellable: record.operation.is_some()
                && matches!(record.phase, GitJobPhase::Queued | GitJobPhase::Running),
            progress_source: record.cancellation.clone(),
        })
    }

    /// 取消当前远程任务。状态保持为取消中，直到进程退出并完成远端确认。
    pub fn cancel_current_job(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self
            .jobs
            .values()
            .filter(|job| {
                job.operation.is_some()
                    && matches!(job.phase, GitJobPhase::Queued | GitJobPhase::Running)
            })
            .map(|job| job.id)
            .min()
        else {
            return;
        };
        let Some(record) = self.jobs.get_mut(&id) else {
            return;
        };
        record.phase = GitJobPhase::Cancelling;
        if let Some(cancellation) = &record.cancellation {
            cancellation.cancel();
        }
        cx.emit(GitStoreEvent::JobsUpdated);
    }

    /// worker 开始执行 job 前标记在途任务（状态栏显示入口）。
    pub(super) fn set_in_flight(&mut self, id: GitJobId, cx: &mut Context<Self>) {
        self.in_flight = Some(id);
        if let Some(record) = self.jobs.get_mut(&id) {
            record.phase = if record
                .cancellation
                .as_ref()
                .is_some_and(GitCancellation::is_cancelled)
            {
                GitJobPhase::Cancelling
            } else {
                GitJobPhase::Running
            };
        }
        cx.emit(GitStoreEvent::JobsUpdated);
    }

    pub(super) fn set_job_phase(
        &mut self,
        id: GitJobId,
        phase: GitJobPhase,
        cx: &mut Context<Self>,
    ) {
        if let Some(record) = self.jobs.get_mut(&id) {
            record.phase = phase;
            cx.emit(GitStoreEvent::JobsUpdated);
        }
    }

    /// 后台执行器结束任务后清空在途标记；旧任务编号不能清掉后来任务的状态。
    pub(super) fn clear_in_flight(&mut self, id: GitJobId, cx: &mut Context<Self>) {
        if self.in_flight == Some(id) {
            self.in_flight = None;
        }
        cx.emit(GitStoreEvent::JobsUpdated);
    }

    pub(super) fn finish_job(&mut self, id: GitJobId, cx: &mut Context<Self>) {
        let Some(record) = self.jobs.remove(&id) else {
            return;
        };
        if self.pending_jobs.get(&record.key) == Some(&id) {
            self.pending_jobs.remove(&record.key);
        }
        if self.in_flight == Some(id) {
            self.in_flight = None;
        }
        cx.emit(GitStoreEvent::JobsUpdated);
    }
}
