//! git 状态编排：仓库发现、status 扫描与增量刷新、事件分发。
//!
//! Git 层的命令全部同步阻塞，这里负责把它们调度到后台线程，并维护每个仓库的状态快照。
//! 后台执行与扫描/合并纯函数在 [`background`] 子模块（可脱离 gpui 单测）。
//!
//! 刷新策略：
//! - 全量（`ReloadGitState`）：仓库发现 + 每个仓库 head/status/双 diff_stat 全扫；
//! - 增量（`RefreshStatuses`）：只对变更路径重查，合并进旧快照；仅当批次含 `.git` 路径时才顺带重读head/branch（外部 checkout 只触发 fs 事件走增量路径，不重读会滞后；纯文件变化走快路径不重读）。
//!
//! 同 key 的排队 job 直接丢弃。

mod background;
mod diff_coordinator;
mod jobs;
mod snapshots;

pub use diff_coordinator::DiffRequest;
use diff_coordinator::{DiffCoordinator, HunkState};
use jobs::{GitJob, GitJobId, GitJobKey, GitJobRecord, ScheduledGitJob};
pub use jobs::{GitJobPhase, GitJobStatus, GitOperationKind};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use background::{JobResult, execute_job, repo_relative_path};
use gpui::{App, AsyncApp, BackgroundExecutor, Context, EventEmitter, Task, WeakEntity};
use zcv_git::{
    Branch, DiffBase, DiffStat, FileStatus, GitCancellation, GitRepository, GitRevision,
};
use zcv_git::{DiffHunk, GitHunkOperation};

/// 一次增量刷新最多累积的路径数，超过则升级为全量扫描。
const MAX_INCREMENTAL_PATHS: usize = 500;

/// GitStore 通知事件。
///
/// 单窗口简化：事件均无 payload（除 `Uncommitted`），订阅方收到后按需重读 GitStore 状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitStoreEvent {
    /// 仓库集合发生变化（发现/消失）。
    Repositories,
    /// 文件状态或 diff 统计发生变化。
    Statuses,
    /// 打开文件的行级差异状态发生变化。
    HunksChanged,
    /// 当前分支、HEAD 或分支列表发生变化。
    Head,
    /// 活动仓库变化（跟随焦点文件切换；订阅方重读 `current_branch()`，无需 payload）。
    ActiveRepositoryChanged,
    /// 后台 job 集合变化（开始/完成/取消）；订阅方重读 `current_job()`。
    JobsUpdated,
    /// 撤销提交成功：携带被撤销的提交消息（面板填回提交信息编辑器）。
    Uncommitted(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitOperationOutcome {
    Completed,
    Cancelled,
    CompletedBeforeCancellation,
    CancellationUnconfirmed(String),
    Failed(String),
}

/// 单个文件在某个仓库中的状态快照。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub status: FileStatus,
    /// 暂存 + 未暂存之和（面板展示改动计数用）。
    pub diff_stat: DiffStat,
    pub staged_diff_stat: DiffStat,
    pub unstaged_diff_stat: DiffStat,
}

/// 活动仓库的远程操作状态（remote 配置与 upstream 领先/落后计数）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoteOperationState {
    /// 是否配置了 remote（无 remote 时 fetch/pull/push 均不可用）。
    pub has_remote: bool,
    /// 本地领先 upstream 的提交数（可推送数）。
    pub ahead: usize,
    /// 本地落后 upstream 的提交数（可拉取数）。
    pub behind: usize,
    /// 当前远程操作及其阶段；存在时同步、拉取、推送入口全部禁用。
    pub operation: Option<GitOperationKind>,
    pub phase: Option<GitJobPhase>,
}

/// 单个仓库的状态快照。
#[derive(Debug)]
pub struct RepositorySnapshot {
    pub branch: Option<String>,
    pub head: Option<String>,
    /// 最近一次提交的 subject（首行；无提交时为 None）。
    /// 底部提交区显示用，status 扫描时顺手读取 `%(contents:subject)`。
    pub last_commit_message: Option<String>,
    /// 是否配置了 remote。
    pub has_remote: bool,
    /// 当前分支相对 upstream 的领先/落后计数（无 upstream 时为 0）。
    pub ahead: usize,
    pub behind: usize,
    /// 本地分支列表（分支选择器数据源；空仓库为空列表）。
    pub branch_list: Vec<Branch>,
    /// 相对仓库根的路径 → 状态。
    pub statuses_by_path: BTreeMap<PathBuf, StatusEntry>,
}

/// 可脱离 GPUI 的 Git 状态只读快照，供项目树后台计算可见行状态。
///
/// GitStore 仍是唯一状态所有者；该类型只是一次不可变的派生视图，不参与写回。
#[derive(Clone, Default)]
pub struct GitStatusSnapshot {
    repositories: Vec<GitRepositoryStatusSnapshot>,
}

#[derive(Clone)]
struct GitRepositoryStatusSnapshot {
    working_directory: PathBuf,
    statuses_by_path: BTreeMap<PathBuf, StatusEntry>,
    directory_statuses: BTreeMap<PathBuf, FileStatus>,
}

fn directory_statuses(statuses: &BTreeMap<PathBuf, StatusEntry>) -> BTreeMap<PathBuf, FileStatus> {
    let mut directories = BTreeMap::new();
    for (path, entry) in statuses {
        if entry.status.is_ignored() {
            continue;
        }
        let mut parent = path.parent().map(Path::to_path_buf);
        while let Some(directory) = parent
            .as_deref()
            .filter(|directory| !directory.as_os_str().is_empty())
        {
            let directory = directory.to_path_buf();
            directories
                .entry(directory.clone())
                .and_modify(|current: &mut FileStatus| {
                    if entry.status.priority() > current.priority() {
                        *current = entry.status;
                    }
                })
                .or_insert(entry.status);
            parent = directory.parent().map(Path::to_path_buf);
        }
    }
    directories
}

impl GitStatusSnapshot {
    pub(crate) fn statuses_for_rows(
        &self,
        rows: &[(PathBuf, bool)],
    ) -> HashMap<PathBuf, FileStatus> {
        rows.iter()
            .filter_map(|(path, is_dir)| {
                // Worktree 与 Git 实现可能返回不同形式的绝对路径（例如 macOS 的 /var 与 /private/var 别名）；
                // 归一化只发生在后台索引查询。
                let canonical_path = canonicalize_path(path);
                let status = if *is_dir {
                    self.status_for_directory(&canonical_path)
                } else {
                    self.status_for_path(&canonical_path)
                        .map(|entry| entry.status)
                };
                status.map(|status| (path.clone(), status))
            })
            .collect()
    }

    fn repository_for_path(&self, path: &Path) -> Option<&GitRepositoryStatusSnapshot> {
        self.repositories
            .iter()
            .filter(|repository| path.starts_with(&repository.working_directory))
            .max_by_key(|repository| repository.working_directory.components().count())
    }

    fn status_for_path(&self, path: &Path) -> Option<&StatusEntry> {
        let repository = self.repository_for_path(path)?;
        let relative = path.strip_prefix(&repository.working_directory).ok()?;
        repository
            .statuses_by_path
            .get(relative)
            .or_else(|| GitStore::ignored_ancestor_entry(&repository.statuses_by_path, relative))
    }

    fn status_for_directory(&self, path: &Path) -> Option<FileStatus> {
        let repository = self.repository_for_path(path)?;
        let relative = path.strip_prefix(&repository.working_directory).ok()?;
        let statuses = &repository.statuses_by_path;
        if let Some(entry) = statuses.get(relative)
            && entry.status.is_ignored()
        {
            return Some(FileStatus::Ignored);
        }
        repository
            .directory_statuses
            .get(relative)
            .copied()
            .or_else(|| {
                GitStore::ignored_ancestor_entry(statuses, relative).map(|entry| entry.status)
            })
    }
}

pub(super) struct Repository {
    repository: Arc<dyn GitRepository>,
    snapshot: RepositorySnapshot,
}

pub struct GitStore {
    /// 项目根目录；无 worktree 的空项目为 None，此时所有 job 与仓库查询为空操作。
    root: Option<PathBuf>,
    repositories: Vec<Repository>,
    /// 当前仓库状态的不可变派生索引；
    /// 项目树只克隆 Arc，不在 UI 线程复制状态表。
    status_index: Arc<GitStatusSnapshot>,
    /// 活动仓库（按 working_directory 标识）：分支显示与 fetch/pull/push 等 git 操作的目标。
    /// 用 working_directory 而非索引：全量扫描重建 Vec，索引不稳定。
    active_repo_workdir: Option<PathBuf>,
    /// HEAD/index 文本缓存；状态或 HEAD 变化时失效。
    revision_text_cache: HashMap<(GitRevision, PathBuf), Arc<str>>,
    /// 分修订递增的缓存版本；失效前启动的后台读取不得回填新缓存。
    revision_text_generations: HashMap<GitRevision, u64>,
    background: BackgroundExecutor,
    /// 自身弱句柄：后台任务完成后回填缓存等状态用（构造时注入）。
    self_handle: WeakEntity<Self>,
    job_sender: async_channel::Sender<ScheduledGitJob>,
    next_job_id: GitJobId,
    pending_jobs: HashMap<GitJobKey, GitJobId>,
    jobs: HashMap<GitJobId, GitJobRecord>,
    in_flight: Option<GitJobId>,
    paths_needing_status_update: BTreeSet<PathBuf>,
    diff_coordinator: DiffCoordinator,
    _job_task: Task<()>,
}

impl GitStore {
    pub fn new(root: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        // 仓库的 working_directory 来自 canonicalize，root 同样归一化，保证路径前缀匹配一致。
        let root = root.map(|root| canonicalize_path(&root));
        let background = cx.background_executor().clone();
        let (job_sender, job_receiver) = async_channel::unbounded::<ScheduledGitJob>();
        // 单 worker 循环（照 fs_task 先例）：顺序处理 job，每个 job 在后台线程执行 git 命令，结果提交回 UI 线程。
        let job_task = cx.spawn(|this: WeakEntity<Self>, asynccx: &mut AsyncApp| {
            let mut cx = asynccx.clone();
            async move {
                while let Ok(scheduled) = job_receiver.recv().await {
                    let Some(this) = this.upgrade() else {
                        break;
                    };
                    let job = scheduled.job.clone();
                    // 排队期间取消：不启动进程，直接完成这个具体任务编号。
                    if scheduled
                        .cancellation
                        .as_ref()
                        .is_some_and(GitCancellation::is_cancelled)
                    {
                        if let GitJob::GitOperation {
                            on_done: Some(tx), ..
                        } = &job
                        {
                            let _ = tx.send(GitOperationOutcome::Cancelled).await;
                        }
                        let _ =
                            this.update(&mut cx, |store, cx| store.finish_job(scheduled.id, cx));
                        continue;
                    }
                    let Some(prepared) = this
                        .update(&mut cx, |store, _| store.prepare_job(&job))
                        .ok()
                        .flatten()
                    else {
                        let _ =
                            this.update(&mut cx, |store, cx| store.finish_job(scheduled.id, cx));
                        continue;
                    };
                    // 标记在途任务（状态栏开始显示）。
                    let _ = this.update(&mut cx, |store, cx| store.set_in_flight(scheduled.id, cx));
                    let repositories_for_reconciliation = prepared.repositories.clone();
                    let result = cx
                        .background_executor()
                        .spawn(execute_job(
                            prepared.root,
                            job.clone(),
                            prepared.repositories,
                            prepared.grouped_paths,
                            prepared.grouped_diff_requests,
                            scheduled.cancellation.clone(),
                        ))
                        .await;

                    let cancelled = scheduled
                        .cancellation
                        .as_ref()
                        .is_some_and(GitCancellation::is_cancelled);
                    let mut cancelled_outcome = None;
                    if cancelled && let GitJob::GitOperation { operation, .. } = &job {
                        let _ = this.update(&mut cx, |store, cx| {
                            store.set_job_phase(scheduled.id, GitJobPhase::Reconciling, cx)
                        });
                        let repository = repositories_for_reconciliation.first().cloned();
                        let operation = *operation;
                        cancelled_outcome = Some(
                            cx.background_executor()
                                .spawn(async move {
                                    background::reconcile_cancelled_operation(operation, repository)
                                })
                                .await,
                        );
                    }
                    let _ =
                        this.update(&mut cx, |store, cx| store.clear_in_flight(scheduled.id, cx));
                    // 操作结果回传发起方（Workspace await 后直接弹提示）。
                    if let GitJob::GitOperation {
                        on_done: Some(tx), ..
                    } = &job
                    {
                        let outcome = if let Some(outcome) = cancelled_outcome {
                            outcome
                        } else {
                            match &result {
                                JobResult::GitOperation(op_result) => match op_result {
                                    Ok(()) => GitOperationOutcome::Completed,
                                    Err(error) => GitOperationOutcome::Failed(format!("{error:#}")),
                                },
                                _ => GitOperationOutcome::Completed,
                            }
                        };
                        let _ = tx.send(outcome).await;
                    }
                    let _ = this.update(&mut cx, |store, cx| {
                        // 不合并已取消命令的部分结果，后续确认与全量扫描才是状态真值来源。
                        if cancelled {
                            store.schedule_scan(cx);
                        } else {
                            store.commit_job(&job, result, cx);
                        }
                        store.finish_job(scheduled.id, cx);
                        if matches!(job, GitJob::RefreshHunks) {
                            // 当前批次运行期间到达的新版本留在等待集合，旧任务移除后立即续开下一批。
                            store.schedule_pending_hunks(cx);
                        }
                    });
                }
            }
        });

        let self_handle = cx.weak_entity();
        Self {
            root,
            repositories: Vec::new(),
            status_index: Arc::new(GitStatusSnapshot::default()),
            active_repo_workdir: None,
            revision_text_cache: HashMap::new(),
            revision_text_generations: HashMap::from([
                (GitRevision::Head, 1),
                (GitRevision::Index, 1),
            ]),
            background,
            self_handle,
            job_sender,
            next_job_id: 1,
            pending_jobs: HashMap::new(),
            jobs: HashMap::new(),
            in_flight: None,
            paths_needing_status_update: BTreeSet::new(),
            diff_coordinator: DiffCoordinator::new(),
            _job_task: job_task,
        }
    }

    /// 全量扫描：重新发现仓库并重扫所有状态（初始扫描与结构性变化时调用）。
    pub(super) fn schedule_scan(&mut self, cx: &mut Context<Self>) {
        self.paths_needing_status_update.clear();
        self.schedule_job(GitJob::ReloadGitState, cx);
    }

    /// 后台执行用户触发的 git 操作（fetch/pull/push），完成后重新扫描。
    ///
    /// 返回的结果由发起方 await 后自行提示（操作发起方持有结果）。
    /// 仓库尚未扫描完成（首次打开项目）时只触发扫描，任务立即以错误结束。
    pub fn run_operation(
        &mut self,
        operation: GitOperationKind,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<GitOperationOutcome>> {
        if self.repositories.is_empty() {
            eprintln!(
                "git 仓库尚未就绪，跳过 {:?}（等待首次扫描完成后重试）",
                operation
            );
            self.schedule_scan(cx);
            return Task::ready(Err(anyhow::anyhow!("git 仓库尚未就绪")));
        }
        // 所有远程操作共享一个闸门，避免凭据弹窗、ref 更新与重试互相竞争。
        if self.jobs.values().any(|job| job.operation.is_some()) {
            return Task::ready(Err(anyhow::anyhow!("已有远程操作正在进行")));
        }
        let (result_tx, result_rx) = async_channel::unbounded::<GitOperationOutcome>();
        self.schedule_job(
            GitJob::GitOperation {
                operation,
                on_done: Some(result_tx),
            },
            cx,
        );
        cx.spawn(|_this: WeakEntity<Self>, _cx: &mut AsyncApp| async move {
            result_rx
                .recv()
                .await
                .map_err(|_| anyhow::anyhow!("远程操作结果通道已关闭"))
        })
    }

    /// 在项目根初始化 git 仓库（空态面板按钮触发），完成后重新扫描以发现新仓库。
    pub fn git_init(&mut self, cx: &mut Context<Self>) {
        self.schedule_job(GitJob::GitInit, cx);
    }

    /// 暂存路径（面板复选框勾选触发；`git update-index`），完成后自动重新扫描。
    pub fn stage_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.schedule_job(GitJob::StageFiles { stage: true, paths }, cx);
    }

    /// 取消暂存路径（面板复选框取消勾选触发；`git reset`），完成后自动重新扫描。
    pub fn unstage_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.schedule_job(
            GitJob::StageFiles {
                stage: false,
                paths,
            },
            cx,
        );
    }

    /// 对单个 hunk 执行暂存、取消暂存或工作区还原，完成后重扫 Git 状态。
    pub fn apply_hunk(
        &mut self,
        operation: GitHunkOperation,
        path: PathBuf,
        hunk: DiffHunk,
        cx: &mut Context<Self>,
    ) {
        self.schedule_job(
            GitJob::HunkOperation {
                operation,
                path,
                hunk,
            },
            cx,
        );
    }

    /// 提交暂存内容（消息来自面板提交信息编辑器）。
    ///
    /// 成功后重扫，Head/Statuses 事件驱动面板清空编辑器并刷新上次提交信息。
    pub fn commit(&mut self, message: String, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            eprintln!("git 仓库尚未就绪，跳过 commit（等待首次扫描完成后重试）");
            self.schedule_scan(cx);
            return;
        }
        if !self.has_staged_changes() {
            return;
        }
        self.schedule_job(GitJob::Commit { message }, cx);
    }

    /// 撤销最近一次提交（`git reset --soft HEAD^`），被撤销消息填回提交信息编辑器。
    pub fn uncommit(&mut self, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            eprintln!("git 仓库尚未就绪，跳过 uncommit（等待首次扫描完成后重试）");
            self.schedule_scan(cx);
            return;
        }
        self.schedule_job(GitJob::Uncommit, cx);
    }

    /// 切换活动仓库到指定本地分支（分支选择器确认触发），完成后自动重扫。
    pub fn checkout_branch(&mut self, name: String, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            eprintln!("git 仓库尚未就绪，跳过 checkout（等待首次扫描完成后重试）");
            self.schedule_scan(cx);
            return;
        }
        self.schedule_job(GitJob::CheckoutBranch { name }, cx);
    }

    /// 以当前 HEAD 为基创建并切换分支（选择器"创建分支"行触发），完成后自动重扫。
    pub fn create_branch(&mut self, name: String, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            eprintln!("git 仓库尚未就绪，跳过 create_branch（等待首次扫描完成后重试）");
            self.schedule_scan(cx);
            return;
        }
        self.schedule_job(GitJob::CreateBranch { name }, cx);
    }

    /// 枚举所有仓库（working_directory → 快照），顺序 = 发现顺序（祖先前置）。
    ///
    /// 返回借用，调用方按需读取字段；面板行模型构建的直接数据源。
    pub fn repositories(&self) -> impl Iterator<Item = (&Path, &RepositorySnapshot)> {
        self.repositories.iter().map(|repository| {
            (
                repository.repository.working_directory(),
                &repository.snapshot,
            )
        })
    }

    /// 捕获当前 Git 状态快照，供后台消费者按自己的可见行集合派生状态。
    pub fn status_snapshot(&self) -> Arc<GitStatusSnapshot> {
        Arc::clone(&self.status_index)
    }

    pub(super) fn rebuild_status_index(&mut self) {
        self.status_index = Arc::new(GitStatusSnapshot {
            repositories: self
                .repositories
                .iter()
                .map(|repository| {
                    let statuses_by_path = repository.snapshot.statuses_by_path.clone();
                    GitRepositoryStatusSnapshot {
                        working_directory: repository.repository.working_directory().to_path_buf(),
                        directory_statuses: directory_statuses(&statuses_by_path),
                        statuses_by_path,
                    }
                })
                .collect(),
        });
    }

    /// 增量刷新：对变更路径重查状态（fs 事件、保存操作后调用）。
    pub fn refresh_statuses_for_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        // 调用方传入的路径可能未 canonicalize，与归一化后的 root 比较前先归一化。
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
        self.paths_needing_status_update.extend(paths);
        // 路径累积超过阈值时升级为全量扫描，避免单次增量 job 过大。
        if self.paths_needing_status_update.len() >= MAX_INCREMENTAL_PATHS {
            self.schedule_scan(cx);
        } else {
            self.schedule_job(GitJob::RefreshStatuses, cx);
        }
    }

    /// 查询文件状态（最长前缀匹配仓库；不在任何仓库中时为 None）。
    ///
    /// 自身无条目时继承最近祖先目录的忽略状态：`--ignored=matching` 对整棵被忽略子树只报告目录级条目，子树内的路径没有条目；
    /// 不传播的话被忽略目录展开后，其子项会显示为"无状态"，与 Zed 快照的子树忽略传播不一致。
    pub(super) fn status_for_path(&self, path: &Path) -> Option<&StatusEntry> {
        let path = canonicalize_path(path);
        let repository = self.repo_for_path(&path)?;
        let relative = repo_relative_path(repository.repository.working_directory(), &path)?;
        let statuses = &repository.snapshot.statuses_by_path;
        statuses
            .get(&relative)
            .or_else(|| Self::ignored_ancestor_entry(statuses, &relative))
    }

    /// 文件的行级差异。`None` 只表示当前版本正在等待结果；无需查询与失败都返回空集合。
    pub fn hunks_for_path(&self, base: DiffBase, path: &Path) -> Option<Arc<[DiffHunk]>> {
        let request = DiffRequest::new(base, canonicalize_path(path));
        match self
            .diff_coordinator
            .records
            .get(&request)
            .map(|record| &record.state)
        {
            Some(HunkState::Ready(hunks)) => Some(hunks.clone()),
            Some(HunkState::NotNeeded) => Some(Arc::from([])),
            Some(HunkState::Failed(error)) => {
                let _ = error;
                Some(Arc::from([]))
            }
            Some(HunkState::Unloaded | HunkState::Queued | HunkState::Loading) => None,
            None if self.request_needs_hunks(&request) => None,
            None => Some(Arc::from([])),
        }
    }

    /// 查找最近一个被忽略的祖先目录条目；自身无条目时用于继承忽略状态。
    ///
    /// 只认 Ignored 条目：祖先链上命中的首个目录条目若不是忽略（例如子树内被负向规则放行的路径，git 会为相关路径生成条目），不向下继承。
    fn ignored_ancestor_entry<'a>(
        statuses: &'a BTreeMap<PathBuf, StatusEntry>,
        relative: &Path,
    ) -> Option<&'a StatusEntry> {
        let mut ancestor = relative.parent();
        while let Some(dir) = ancestor {
            if let Some(entry) = statuses.get(dir)
                && entry.status.is_ignored()
            {
                return Some(entry);
            }
            ancestor = dir.parent();
        }
        None
    }

    /// 当前活动仓库的分支名（无仓库、active 未建立或活动仓库为空仓库时为 None）。
    pub fn current_branch(&self) -> Option<&str> {
        self.active_repo_workdir
            .as_ref()
            .and_then(|workdir| self.repo_by_workdir(workdir))
            .and_then(|repository| repository.snapshot.branch.as_deref())
    }

    /// 当前活动仓库 HEAD 提交的完整 oid（detached HEAD 时存在；空仓库无提交时为 None）。
    pub fn current_head_commit(&self) -> Option<&str> {
        self.active_repo_workdir
            .as_ref()
            .and_then(|workdir| self.repo_by_workdir(workdir))
            .and_then(|repository| repository.snapshot.head.as_deref())
    }

    /// 全部仓库变更行数汇总（staged 与 unstaged 合并计数），版本控制面板顶部统计用。
    pub fn total_diff_stat(&self) -> DiffStat {
        let mut total = DiffStat::default();
        for repository in &self.repositories {
            for entry in repository.snapshot.statuses_by_path.values() {
                if entry.status.is_ignored() {
                    continue;
                }
                total.added += entry.staged_diff_stat.added + entry.unstaged_diff_stat.added;
                total.deleted += entry.staged_diff_stat.deleted + entry.unstaged_diff_stat.deleted;
            }
        }
        total
    }

    /// 活动仓库的本地分支列表（无仓库、active 未建立时为 None；空仓库为空列表）。
    ///
    /// 与 current_branch 同仓库选择策略，保证分支按钮与列表一致。
    pub fn active_branch_list(&self) -> Option<&[Branch]> {
        self.active_repo_workdir
            .as_ref()
            .and_then(|workdir| self.repo_by_workdir(workdir))
            .map(|repository| repository.snapshot.branch_list.as_slice())
    }

    /// 活动仓库最近一次提交的 subject（底部提交区显示）。
    ///
    /// 仓库选择与 fetch/pull/push、提交目标一致（`active_repository`），保证"显示的提交信息"与"提交目标仓库"对齐。
    pub fn last_commit_message(&self) -> Option<&str> {
        self.active_repository()
            .and_then(|repository| repository.snapshot.last_commit_message.as_deref())
    }

    /// 活动仓库是否存在已暂存改动（提交按钮与提交动作的资格判断来源）。
    pub fn has_staged_changes(&self) -> bool {
        self.active_repository().is_some_and(|repository| {
            repository
                .snapshot
                .statuses_by_path
                .values()
                .any(|entry| entry.status.has_staged())
        })
    }

    /// 按 working_directory 查找仓库。
    fn repo_by_workdir(&self, workdir: &Path) -> Option<&Repository> {
        self.repositories
            .iter()
            .find(|repository| repository.repository.working_directory() == workdir)
    }

    /// 操作目标仓库：active 已建立时用它，否则回退「首个有分支的仓库」→「首个」。
    ///
    /// fetch/pull/push、提交、uncommit 与底部提交信息显示共用此选择（操作以 active 仓库为目标，空仓库也执行）。
    fn active_repository(&self) -> Option<&Repository> {
        self.active_repo_workdir
            .as_ref()
            .and_then(|workdir| self.repo_by_workdir(workdir))
            .or_else(|| {
                self.repositories
                    .iter()
                    .find(|repository| repository.snapshot.branch.is_some())
            })
            .or_else(|| self.repositories.first())
    }

    /// 活动仓库的远程操作状态（可推送/可拉取判定依据）。
    pub fn remote_operation_state(&self) -> RemoteOperationState {
        let active_operation = self
            .jobs
            .values()
            .filter_map(|job| {
                job.operation
                    .map(|operation| (job.id, operation, job.phase))
            })
            .min_by_key(|(id, _, _)| *id);
        let mut state = self
            .active_repo_workdir
            .as_ref()
            .and_then(|workdir| self.repo_by_workdir(workdir))
            .map(|repository| RemoteOperationState {
                has_remote: repository.snapshot.has_remote,
                ahead: repository.snapshot.ahead,
                behind: repository.snapshot.behind,
                operation: None,
                phase: None,
            })
            .unwrap_or_default();
        if let Some((_, operation, phase)) = active_operation {
            state.operation = Some(operation);
            state.phase = Some(phase);
        }
        state
    }

    /// 按路径更新活动仓库（最长前缀匹配；焦点文件切换时由 Workspace 调用）。
    ///
    /// 路径可能未 canonicalize（如设置文件入口），先归一化再匹配；
    /// 路径不在任何仓库中（如已删除）时保持当前活动仓库不变。
    pub fn set_active_repository_for_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let path = canonicalize_path(path);
        let Some(repository) = self.repo_for_path(&path) else {
            return;
        };
        let workdir = repository.repository.working_directory().to_path_buf();
        if self.active_repo_workdir.as_deref() != Some(workdir.as_path()) {
            self.active_repo_workdir = Some(workdir);
            cx.emit(GitStoreEvent::ActiveRepositoryChanged);
        }
    }

    /// 是否已发现至少一个 git 仓库（决定 git 相关 UI 是否可见）。
    pub fn has_repositories(&self) -> bool {
        !self.repositories.is_empty()
    }

    /// 读取 HEAD 或 index 中 `path` 的文本并回填缓存。
    ///
    /// 缓存生命周期全部由 GitStore 管理：加载即回填，HEAD 变化时 commit_job 清空。
    pub fn load_revision_text(
        &self,
        revision: GitRevision,
        path: &Path,
        cx: &App,
    ) -> Task<Option<String>> {
        let background = self.background.clone();
        let path = canonicalize_path(path);
        let Some(repository) = self.repo_for_path(&path) else {
            return background.spawn(async { None });
        };
        let repository = repository.repository.clone();
        let generation = self
            .revision_text_generations
            .get(&revision)
            .copied()
            .unwrap_or_default();
        let Some(relative) = repo_relative_path(repository.working_directory(), &path) else {
            return background.spawn(async { None });
        };
        let revision_spec = match revision {
            GitRevision::Head => format!("HEAD:{}", relative.to_string_lossy()),
            GitRevision::Index => format!(":{}", relative.to_string_lossy()),
        };
        let loaded = background.spawn(async move {
            let contents = repository.load_revisions(&[&revision_spec]).ok()?;
            let content = contents.into_iter().next()??;
            Some(String::from_utf8_lossy(&content).into_owned())
        });
        let this = self.self_handle.clone();
        cx.spawn(async move |cx| {
            let text = loaded.await;
            this.update(cx, |store, _| {
                if store.revision_text_generations.get(&revision).copied() == Some(generation) {
                    store.revision_text_cache.insert(
                        (revision, path.clone()),
                        Arc::from(text.clone().unwrap_or_default()),
                    );
                }
            })
            .ok();
            text
        })
    }

    /// 读取缓存的修订文本；`None` 表示尚未完成加载。
    pub fn revision_text(&self, revision: GitRevision, path: &Path) -> Option<Arc<str>> {
        self.revision_text_cache
            .get(&(revision, canonicalize_path(path)))
            .cloned()
    }

    fn invalidate_revision_text(&mut self, revision: GitRevision) {
        self.revision_text_cache
            .retain(|(cached_revision, _), _| *cached_revision != revision);
        let generation = self.revision_text_generations.entry(revision).or_insert(0);
        *generation = generation.wrapping_add(1).max(1);
    }

    fn invalidate_revision_text_for_paths(&mut self, revision: GitRevision, paths: &[PathBuf]) {
        let changed_paths = paths
            .iter()
            .map(|path| canonicalize_path(path))
            .collect::<Vec<_>>();
        self.revision_text_cache
            .retain(|(cached_revision, path), _| {
                *cached_revision != revision
                    || !changed_paths
                        .iter()
                        .any(|changed_path| path.starts_with(changed_path))
            });
        let generation = self.revision_text_generations.entry(revision).or_insert(0);
        *generation = generation.wrapping_add(1).max(1);
    }

    /// UI 线程：取出 job 需要的共享数据（后台线程不能访问 Entity 状态）。
    ///
    /// 无项目根（空工作区）时任何 git job 都无操作对象，直接丢弃。
    fn prepare_job(&mut self, job: &GitJob) -> Option<JobPreparation> {
        let root = self.root.clone()?;
        match job {
            GitJob::ReloadGitState => Some(JobPreparation {
                root,
                repositories: Vec::new(),
                grouped_paths: Vec::new(),
                grouped_diff_requests: Vec::new(),
            }),
            GitJob::RefreshStatuses => {
                let paths: Vec<PathBuf> = std::mem::take(&mut self.paths_needing_status_update)
                    .into_iter()
                    .collect();
                let (repositories, grouped_paths) = self.group_paths_by_repo(&paths);
                Some(JobPreparation {
                    root,
                    repositories,
                    grouped_paths,
                    grouped_diff_requests: Vec::new(),
                })
            }
            GitJob::RefreshHunks => {
                let requests = std::mem::take(&mut self.diff_coordinator.pending);
                let mut repositories = Vec::with_capacity(self.repositories.len());
                let mut grouped_diff_requests = vec![Vec::new(); self.repositories.len()];
                for repository in &self.repositories {
                    repositories.push(repository.repository.clone());
                }
                for (request, generation) in requests {
                    let is_current =
                        self.diff_coordinator
                            .records
                            .get(&request)
                            .is_some_and(|record| {
                                record.generation == generation
                                    && matches!(record.state, HunkState::Queued)
                            });
                    if !is_current {
                        continue;
                    }
                    if !self.request_needs_hunks(&request) {
                        if let Some(record) = self.diff_coordinator.records.get_mut(&request) {
                            record.state = HunkState::NotNeeded;
                        }
                        continue;
                    }
                    let Some((index, relative)) = self
                        .repositories
                        .iter()
                        .enumerate()
                        .filter_map(|(index, repository)| {
                            let workdir = repository.repository.working_directory();
                            repo_relative_path(workdir, &request.path)
                                .map(|relative| (index, workdir.components().count(), relative))
                        })
                        .max_by_key(|(_, depth, _)| *depth)
                        .map(|(index, _, relative)| (index, relative))
                    else {
                        if let Some(record) = self.diff_coordinator.records.get_mut(&request) {
                            record.state = HunkState::Failed("文件所属仓库已不可用".into());
                        }
                        continue;
                    };
                    if let Some(record) = self.diff_coordinator.records.get_mut(&request) {
                        record.state = HunkState::Loading;
                    }
                    self.diff_coordinator
                        .in_flight
                        .insert(request.clone(), generation);
                    grouped_diff_requests[index].push(DiffRequest::new(request.base, relative));
                }
                Some(JobPreparation {
                    root,
                    repositories,
                    grouped_paths: Vec::new(),
                    grouped_diff_requests,
                })
            }
            GitJob::GitOperation { .. }
            | GitJob::CheckoutBranch { .. }
            | GitJob::CreateBranch { .. } => {
                // 作用于活动仓库（fetch/pull/push 以 active 仓库为目标，空仓库也执行；
                // 分支操作与 top_bar 显示的分支同仓库）。
                let repository = self.active_repository()?.repository.clone();
                Some(JobPreparation {
                    root,
                    repositories: vec![repository],
                    grouped_paths: Vec::new(),
                    grouped_diff_requests: Vec::new(),
                })
            }
            // init 作用于项目根，不依赖既有仓库集合。
            GitJob::GitInit => Some(JobPreparation {
                root,
                repositories: Vec::new(),
                grouped_paths: Vec::new(),
                grouped_diff_requests: Vec::new(),
            }),
            GitJob::StageFiles { stage, paths } => {
                let (repositories, grouped_paths) = self.group_paths_by_repo(paths);
                // 目录路径展开为该仓库快照内状态匹配的文件（git update-index 不递归目录，直接传目录会失败；
                // 目录勾选收集其下文件路径逐个暂存）。
                // 只保留与操作方向一致的文件：reset 命中未跟踪路径会报错，且避免误暂存无关文件。
                let grouped_paths = grouped_paths
                    .into_iter()
                    .enumerate()
                    .map(|(index, rel_paths)| {
                        let statuses = &self.repositories[index].snapshot.statuses_by_path;
                        let mut expanded = Vec::new();
                        for rel in rel_paths {
                            let matches = |entry: &StatusEntry| {
                                if *stage {
                                    entry.status.has_unstaged()
                                } else {
                                    entry.status.has_staged()
                                }
                            };
                            match statuses.get(&rel) {
                                Some(entry) if matches(entry) => expanded.push(rel),
                                Some(_) => {}
                                None => expanded.extend(
                                    statuses
                                        .iter()
                                        .filter(|(path, entry)| {
                                            path.starts_with(&rel) && matches(entry)
                                        })
                                        .map(|(path, _)| path.clone()),
                                ),
                            }
                        }
                        expanded
                    })
                    .collect();
                Some(JobPreparation {
                    root,
                    repositories,
                    grouped_paths,
                    grouped_diff_requests: Vec::new(),
                })
            }
            GitJob::HunkOperation { path, .. } => {
                let (repositories, grouped_paths) =
                    self.group_paths_by_repo(std::slice::from_ref(path));
                Some(JobPreparation {
                    root,
                    repositories,
                    grouped_paths,
                    grouped_diff_requests: Vec::new(),
                })
            }
            // 提交/撤销提交：作用于活动仓库（与 GitOperation 同选择策略）。
            GitJob::Commit { .. } | GitJob::Uncommit => {
                let repository = self.active_repository()?;
                Some(JobPreparation {
                    root,
                    repositories: vec![repository.repository.clone()],
                    grouped_paths: Vec::new(),
                    grouped_diff_requests: Vec::new(),
                })
            }
        }
    }

    /// UI 线程：提交 job 结果，比对旧快照后发出对应事件。
    /// 最长前缀匹配仓库（调用方保证路径已 canonicalize）。
    fn repo_for_path(&self, path: &Path) -> Option<&Repository> {
        self.repositories
            .iter()
            .filter(|repository| path.starts_with(repository.repository.working_directory()))
            .max_by_key(|repository| repository.repository.working_directory().as_os_str().len())
    }

    /// 按仓库分组路径（最长前缀匹配），返回 (仓库列表, 每仓库的相对路径组)。
    ///
    /// 路径与仓库根都先归一化，保证前缀比较一致；不在任何仓库内的路径丢弃。
    fn group_paths_by_repo(
        &self,
        paths: &[PathBuf],
    ) -> (Vec<Arc<dyn GitRepository>>, Vec<Vec<PathBuf>>) {
        let mut repositories = Vec::with_capacity(self.repositories.len());
        let mut grouped_paths = vec![Vec::new(); self.repositories.len()];
        for (index, repository) in self.repositories.iter().enumerate() {
            repositories.push(repository.repository.clone());
            let workdir = repository.repository.working_directory();
            grouped_paths[index].extend(paths.iter().filter_map(|path| {
                // fs 事件路径可能未 canonicalize（如 macOS 的 /var → /private/var）。
                let path = canonicalize_path(path);
                path.starts_with(workdir)
                    .then(|| repo_relative_path(workdir, &path))
                    .flatten()
            }));
        }
        (repositories, grouped_paths)
    }
}

/// 路径归一化（canonicalize 失败时保留原样，如路径已删除）。
pub(super) fn canonicalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

impl EventEmitter<GitStoreEvent> for GitStore {}

struct JobPreparation {
    root: PathBuf,
    repositories: Vec<Arc<dyn GitRepository>>,
    grouped_paths: Vec<Vec<PathBuf>>,
    grouped_diff_requests: Vec<Vec<DiffRequest>>,
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    use super::*;
    use crate::git_store::diff_coordinator::HunkRecord;
    use crate::test_support::{rev_parse, run_git, test_git_repo};

    use gpui::AppContext;
    use zcv_git::StatusCode;

    impl GitStore {
        fn status_for_directory(&self, path: &Path) -> Option<FileStatus> {
            self.status_index
                .status_for_directory(&canonicalize_path(path))
        }
    }

    #[gpui::test]
    fn scan_discovers_repository_and_reports_status(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        fs::write(root.join("tracked.txt"), "已修改\n").expect("应修改文件");

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let entry = cx.read_entity(&git_store, |store, _| {
            store.status_for_path(&root.join("tracked.txt")).cloned()
        });
        let entry = entry.expect("应有 tracked.txt 的状态");
        assert!(entry.status.is_modified());
        assert!(entry.diff_stat.added >= 1);

        // 汇总行数：未暂存修改计 2 增（第一行改写为已修改 → 1 删 1 增？numstat 按行粒度），
        // 这里只断言 added 不为零且 deleted 反映改写。
        let total = cx.read_entity(&git_store, |store, _| store.total_diff_stat());
        assert!(total.added >= 1, "未暂存新增应计入汇总");
    }

    #[gpui::test]
    fn total_diff_stat_merges_staged_and_unstaged(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        // 先做一次未暂存修改（1 增 1 删：改写第二行内容）。
        std::fs::write(root.join("tracked.txt"), "第一行\n第二行（改）\n").expect("应写入文件");
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();
        let total = cx.read_entity(&git_store, |store, _| store.total_diff_stat());
        assert_eq!((total.added, total.deleted), (1, 1), "未暂存 1 增 1 删");

        // 暂存后再次修改：staged 与 unstaged 各计一份，合并计数。
        run_git(&root, &["add", "tracked.txt"]);
        std::fs::write(root.join("tracked.txt"), "第一行\n第二行（改）\n第三行\n")
            .expect("应写入文件");
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();
        let total = cx.read_entity(&git_store, |store, _| store.total_diff_stat());
        assert_eq!((total.added, total.deleted), (2, 1), "暂存与未暂存合并计数");

        let branch = cx.read_entity(&git_store, |store, _| {
            store.current_branch().map(str::to_string)
        });
        assert_eq!(branch.as_deref(), Some("master"));
    }

    #[gpui::test]
    fn empty_repository_reports_no_branch(cx: &mut gpui::TestAppContext) {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        run_git(temp_dir.path(), &["init", "-q", "-b", "master"]);

        let git_store =
            cx.update(|cx| cx.new(|cx| GitStore::new(Some(temp_dir.path().to_path_buf()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let branch = cx.read_entity(&git_store, |store, _| {
            store.current_branch().map(str::to_string)
        });
        assert!(branch.is_none());
    }

    #[gpui::test]
    fn incremental_refresh_updates_statuses(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 外部修改文件 → 增量刷新。
        fs::write(root.join("tracked.txt"), "第一行\n第二行\n第三行\n").expect("应修改文件");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(&[root.join("tracked.txt")], cx)
        });
        cx.run_until_parked();

        let (status, head) = cx.read_entity(&git_store, |store, _| {
            let entry = store.status_for_path(&root.join("tracked.txt")).cloned();
            (entry, store.current_branch().map(str::to_string))
        });
        let entry = status.expect("应有刷新后的状态");
        assert!(entry.status.is_modified());
        assert_eq!(entry.diff_stat.added, 1);
        assert_eq!(head.as_deref(), Some("master"));

        // 文件恢复原样 → 增量刷新应移除条目。
        fs::write(root.join("tracked.txt"), "第一行\n第二行\n").expect("应还原文件");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(&[root.join("tracked.txt")], cx)
        });
        cx.run_until_parked();
        assert!(
            cx.read_entity(&git_store, |store, _| {
                store.status_for_path(&root.join("tracked.txt")).cloned()
            })
            .is_none()
        );
    }

    #[gpui::test]
    fn external_checkout_updates_head(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        // 第二个分支。
        run_git(&root, &["checkout", "-q", "-b", "feature"]);
        fs::write(root.join("tracked.txt"), "feature 内容\n").expect("应写入");
        run_git(&root, &["commit", "-q", "-am", "feature"]);

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 外部 checkout 回 master：fs 事件同时触发 .git/HEAD 与工作区文件，
        // 增量刷新包含 .git 路径 → 重读 head（快路径只跳过纯文件变化批次）。
        run_git(&root, &["checkout", "-q", "master"]);
        cx.update_entity(&git_store, |store, cx| {
            store
                .refresh_statuses_for_paths(&[root.join("tracked.txt"), root.join(".git/HEAD")], cx)
        });
        cx.run_until_parked();

        let branch = cx.read_entity(&git_store, |store, _| {
            store.current_branch().map(str::to_string)
        });
        assert_eq!(branch.as_deref(), Some("master"));
    }

    #[gpui::test]
    fn load_revision_text_returns_head_content(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 修改工作区文件，HEAD 内容应仍是初始版本。
        fs::write(root.join("tracked.txt"), "已修改\n").expect("应修改文件");
        let path = root.join("tracked.txt");
        // 前台任务由测试调度器驱动（block 只跑后台任务，无法推进）。
        cx.read_entity(&git_store, |store, cx| {
            store.load_revision_text(GitRevision::Head, &path, cx)
        })
        .detach();
        cx.run_until_parked();
        // 加载结果已由 GitStore 自行回填缓存。
        let text = cx.read_entity(&git_store, |store, _| {
            store.revision_text(GitRevision::Head, &path)
        });
        assert_eq!(text.as_deref(), Some("第一行\n第二行\n"));
    }

    #[gpui::test]
    fn load_revision_text_returns_index_content(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let unchanged_path = root.join("unchanged.txt");
        fs::write(&unchanged_path, "未变更内容\n").expect("应写入未变更文件");
        run_git(&root, &["add", "unchanged.txt"]);
        run_git(&root, &["commit", "-q", "-m", "add unchanged"]);
        fs::write(root.join("tracked.txt"), "已暂存内容\n").expect("应写入暂存版本");
        run_git(&root, &["add", "tracked.txt"]);
        fs::write(root.join("tracked.txt"), "工作区内容\n").expect("应写入工作区版本");

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();
        let path = root.join("tracked.txt");
        cx.read_entity(&git_store, |store, cx| {
            store.load_revision_text(GitRevision::Index, &path, cx)
        })
        .detach();
        cx.read_entity(&git_store, |store, cx| {
            store.load_revision_text(GitRevision::Index, &unchanged_path, cx)
        })
        .detach();
        cx.run_until_parked();

        let text = cx.read_entity(&git_store, |store, _| {
            store.revision_text(GitRevision::Index, &path)
        });
        assert_eq!(text.as_deref(), Some("已暂存内容\n"));

        // 状态类型与增删行统计保持不变时，index 内容变化仍必须使缓存失效。
        fs::write(&path, "第二版暂存\n").expect("应更新暂存版本");
        run_git(&root, &["add", "tracked.txt"]);
        fs::write(&path, "第二版工作区\n").expect("应更新工作区版本");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(std::slice::from_ref(&path), cx)
        });
        cx.run_until_parked();
        assert!(
            cx.read_entity(&git_store, |store, _| store
                .revision_text(GitRevision::Index, &path))
                .is_none(),
            "即使状态枚举与行数未变，刷新路径也必须使旧 index 文本失效"
        );
        assert_eq!(
            cx.read_entity(&git_store, |store, _| store
                .revision_text(GitRevision::Index, &unchanged_path)),
            Some(Arc::from("未变更内容\n")),
            "单路径刷新不应使其他文件的 index 文本失效"
        );

        cx.read_entity(&git_store, |store, cx| {
            store.load_revision_text(GitRevision::Index, &path, cx)
        })
        .detach();
        cx.run_until_parked();
        let text = cx.read_entity(&git_store, |store, _| {
            store.revision_text(GitRevision::Index, &path)
        });
        assert_eq!(text.as_deref(), Some("第二版暂存\n"));
    }

    #[gpui::test]
    fn status_for_directory_aggregates_children(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        fs::create_dir_all(root.join("src")).expect("应创建目录");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("应创建文件");
        run_git(&root, &["add", "src/main.rs"]);
        run_git(&root, &["commit", "-q", "-m", "add src"]);
        fs::create_dir_all(root.join("docs")).expect("应创建目录");
        fs::create_dir_all(root.join("empty")).expect("应创建目录");

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 空目录：无子项 → None。
        assert!(
            cx.read_entity(&git_store, |store, _| store
                .status_for_directory(&root.join("empty")))
                .is_none()
        );

        // src 下出现已修改文件 → 目录聚合为 Modified。
        fs::write(root.join("src/main.rs"), "fn main() { println!(); }\n").expect("应修改文件");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(&[root.join("src/main.rs")], cx)
        });
        cx.run_until_parked();
        let src = cx.read_entity(&git_store, |store, _| {
            store.status_for_directory(&root.join("src"))
        });
        assert!(src.is_some_and(|status| status.is_modified()));

        // docs 下只有未跟踪文件 → 目录聚合为 Untracked（优先级低于 modified）。
        fs::write(root.join("docs/note.md"), "note\n").expect("应创建文件");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(&[root.join("docs/note.md")], cx)
        });
        cx.run_until_parked();
        let docs = cx.read_entity(&git_store, |store, _| {
            store.status_for_directory(&root.join("docs"))
        });
        assert!(docs.is_some_and(|status| status.is_untracked()));
        // 同一目录下 modified 与 untracked 并存：modified 优先（优先级更高）。
        fs::write(root.join("src/scratch.rs"), "x\n").expect("应创建文件");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(&[root.join("src/scratch.rs")], cx)
        });
        cx.run_until_parked();
        let src = cx.read_entity(&git_store, |store, _| {
            store.status_for_directory(&root.join("src"))
        });
        assert!(
            src.is_some_and(|status| status.is_modified()),
            "modified 应优先于 untracked"
        );
    }

    #[gpui::test]
    fn status_for_directory_returns_ignored_for_ignored_directory(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        fs::create_dir_all(root.join("node_modules/pkg")).expect("应创建目录");
        fs::write(root.join("node_modules/pkg/index.js"), "x\n").expect("应创建文件");
        fs::write(root.join(".gitignore"), "node_modules/\n").expect("应写入 .gitignore");

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let status = cx.read_entity(&git_store, |store, _| {
            store.status_for_directory(&root.join("node_modules"))
        });
        assert!(status.is_some_and(|status| status.is_ignored()));
    }

    #[gpui::test]
    fn ignored_children_do_not_taint_directory_status(cx: &mut gpui::TestAppContext) {
        // 回归：目录内的忽略文件（如 .DS_Store）不应让目录本身淡显。
        let (root, _temp) = test_git_repo();
        fs::write(root.join(".gitignore"), ".DS_Store\n").expect("应写入 .gitignore");
        fs::create_dir_all(root.join("assets")).expect("应创建目录");
        fs::write(root.join("assets/.DS_Store"), "x").expect("应创建忽略文件");
        fs::write(root.join("assets/logo.png"), "x").expect("应创建文件");
        run_git(&root, &["add", "assets/logo.png"]);
        run_git(&root, &["commit", "-q", "-m", "add assets"]);

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 目录自身未被忽略（无 `!! assets/` 条目），子项只有 Ignored → None。
        let status = cx.read_entity(&git_store, |store, _| {
            store.status_for_directory(&root.join("assets"))
        });
        assert!(
            status.is_none(),
            "仅有忽略子项的目录不应淡显，实际 {status:?}"
        );

        // 子项出现修改后，忽略文件不参与聚合，目录仍显示修改状态。
        fs::write(root.join("assets/logo.png"), "x\nx\n").expect("应修改文件");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(&[root.join("assets/logo.png")], cx)
        });
        cx.run_until_parked();
        let status = cx.read_entity(&git_store, |store, _| {
            store.status_for_directory(&root.join("assets"))
        });
        assert!(status.is_some_and(|status| status.is_modified()));
    }

    #[gpui::test]
    fn ignored_directory_status_propagates_to_descendants(cx: &mut gpui::TestAppContext) {
        // `--ignored=matching` 对整棵被忽略子树只报告目录级条目（如 `!! tmp/`），子树内的文件与目录无条目；
        // 查询时应沿祖先链继承忽略状态。
        let (root, _temp) = test_git_repo();
        fs::write(root.join(".gitignore"), "tmp/\n").expect("应写入 .gitignore");
        fs::create_dir_all(root.join("tmp")).expect("应创建 tmp 目录");
        let file = root.join("tmp/x.txt");
        fs::write(&file, "x\n").expect("应创建被忽略文件");
        let empty_dir = root.join("tmp/empty");
        fs::create_dir_all(&empty_dir).expect("应创建被忽略目录");

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 目录级忽略条目本身。
        assert!(
            cx.read_entity(&git_store, |store, _| {
                store.status_for_directory(&root.join("tmp"))
            })
            .is_some_and(|status| status.is_ignored())
        );
        // 子树内文件与空目录继承忽略状态。
        assert!(
            cx.read_entity(&git_store, |store, _| {
                store.status_for_path(&file).map(|entry| entry.status)
            })
            .is_some_and(|status| status.is_ignored())
        );
        assert!(
            cx.read_entity(&git_store, |store, _| {
                store.status_for_directory(&empty_dir)
            })
            .is_some_and(|status| status.is_ignored())
        );
        // 非忽略路径不受影响（普通未跟踪文件仍为 Untracked）。
        fs::write(root.join("scratch.txt"), "x\n").expect("应创建文件");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(&[root.join("scratch.txt")], cx)
        });
        cx.run_until_parked();
        assert!(
            cx.read_entity(&git_store, |store, _| {
                store
                    .status_for_path(&root.join("scratch.txt"))
                    .map(|entry| entry.status)
            })
            .is_some_and(|status| status.is_untracked())
        );
    }

    #[gpui::test]
    fn run_operation_pushes_to_remote(cx: &mut gpui::TestAppContext) {
        // 工作仓库与裸远程共用 temp_dir，保证测试期间目录存活。
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let remote = temp_dir.path().join("remote.git");
        run_git(
            temp_dir.path(),
            &["init", "-q", "--bare", remote.to_str().unwrap()],
        );
        let root = temp_dir.path().join("work");
        fs::create_dir(&root).expect("应创建工作仓库目录");
        run_git(&root, &["init", "-q", "-b", "master"]);
        run_git(&root, &["config", "user.email", "test@example.com"]);
        run_git(&root, &["config", "user.name", "Test User"]);
        fs::write(root.join("tracked.txt"), "内容\n").expect("应写入初始文件");
        run_git(&root, &["add", "tracked.txt"]);
        run_git(&root, &["commit", "-q", "-m", "initial"]);
        run_git(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_git(&root, &["push", "-q", "-u", "origin", "master"]);

        let git_store = cx.new(|cx| GitStore::new(Some(root.clone()), cx));
        git_store.update(cx, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked(); // 首次扫描完成，repositories 就绪。
        let ready = cx.read_entity(&git_store, |store, _| !store.repositories.is_empty());
        assert!(ready, "首次扫描后 repositories 应就绪");

        // 本地新提交 → run_operation(Push) → 后台 job 推送。
        fs::write(root.join("new.txt"), "新文件\n").expect("应写入文件");
        run_git(&root, &["add", "new.txt"]);
        run_git(&root, &["commit", "-q", "-m", "新提交"]);
        git_store.update(cx, |store, cx| {
            drop(store.run_operation(GitOperationKind::Push, cx));
        });
        cx.run_until_parked();
        let job_done = cx.read_entity(&git_store, |store, _| {
            !store
                .pending_jobs
                .contains_key(&GitJobKey::GitOperation(GitOperationKind::Push))
        });
        assert!(job_done, "push job 应已完成");

        // 远程应指向本地 HEAD。
        let rev = rev_parse;
        assert_eq!(rev(&remote), rev(&root), "push 后远程应指向本地 HEAD");
    }

    #[gpui::test]
    fn cancelling_queued_remote_operation_keeps_gate_closed_until_finished(
        cx: &mut gpui::TestAppContext,
    ) {
        let (root, _temp) = test_git_repo();
        let git_store = cx.new(|cx| GitStore::new(Some(root), cx));
        git_store.update(cx, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        git_store.update(cx, |store, cx| {
            drop(store.run_operation(GitOperationKind::Push, cx));
            let status = store.current_job().expect("推送应立即进入排队状态");
            assert_eq!(status.phase, GitJobPhase::Queued);
            assert_eq!(status.operation, Some(GitOperationKind::Push));

            store.cancel_current_job(cx);
            assert_eq!(
                store.current_job().map(|status| status.phase),
                Some(GitJobPhase::Cancelling)
            );
            let next_job_id = store.next_job_id;
            drop(store.run_operation(GitOperationKind::Fetch, cx));
            assert_eq!(
                store.next_job_id, next_job_id,
                "取消完成前不得启动新远程操作"
            );
            assert_eq!(
                store
                    .jobs
                    .values()
                    .filter(|job| job.operation.is_some())
                    .count(),
                1,
                "取消期间只能保留原任务实例"
            );
        });

        cx.run_until_parked();
        assert!(
            cx.read_entity(&git_store, |store, _| store
                .jobs
                .values()
                .all(|job| job.operation.is_none())),
            "排队任务被 worker 确认取消后应释放远程操作闸门"
        );
    }

    #[gpui::test]
    fn stale_job_completion_does_not_remove_newer_same_key(cx: &mut gpui::TestAppContext) {
        let git_store = cx.new(|cx| GitStore::new(None, cx));
        git_store.update(cx, |store, cx| {
            let key = GitJobKey::GitOperation(GitOperationKind::Push);
            let old_id = store
                .schedule_job(
                    GitJob::GitOperation {
                        operation: GitOperationKind::Push,
                        on_done: None,
                    },
                    cx,
                )
                .expect("旧任务应成功排队");
            let new_id = old_id + 1;
            store.jobs.insert(
                new_id,
                GitJobRecord {
                    id: new_id,
                    key: key.clone(),
                    name: "新推送".into(),
                    operation: Some(GitOperationKind::Push),
                    phase: GitJobPhase::Queued,
                    cancellation: Some(GitCancellation::new()),
                },
            );
            store.pending_jobs.insert(key.clone(), new_id);

            store.finish_job(old_id, cx);
            assert_eq!(store.pending_jobs.get(&key), Some(&new_id));
            assert!(store.jobs.contains_key(&new_id));
        });
    }

    #[cfg(unix)]
    #[gpui::test]
    fn cancelling_running_push_allows_clean_retry_after_process_exit(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let remote = temp_dir.path().join("remote.git");
        run_git(
            temp_dir.path(),
            &["init", "-q", "--bare", remote.to_str().unwrap()],
        );
        let root = temp_dir.path().join("work");
        fs::create_dir(&root).expect("应创建工作仓库目录");
        run_git(&root, &["init", "-q", "-b", "master"]);
        run_git(&root, &["config", "user.email", "test@example.com"]);
        run_git(&root, &["config", "user.name", "Test User"]);
        fs::write(root.join("tracked.txt"), "初始内容\n").expect("应写入初始文件");
        run_git(&root, &["add", "tracked.txt"]);
        run_git(&root, &["commit", "-q", "-m", "initial"]);
        run_git(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_git(&root, &["push", "-q", "-u", "origin", "master"]);
        fs::write(root.join("new.txt"), "待推送内容\n").expect("应写入新文件");
        run_git(&root, &["add", "new.txt"]);
        run_git(&root, &["commit", "-q", "-m", "待取消推送"]);

        let hook_path = root.join(".git/hooks/pre-push");
        fs::write(
            &hook_path,
            "#!/bin/sh\necho '测试推送等待中' >&2\nsleep 30\n",
        )
        .expect("应写入 pre-push 钩子");
        let mut permissions = fs::metadata(&hook_path)
            .expect("应读取钩子权限")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook_path, permissions).expect("应设置钩子可执行权限");

        let git_store = cx.new(|cx| GitStore::new(Some(root.clone()), cx));
        git_store.update(cx, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let started = Instant::now();
        let cancellation = git_store.update(cx, |store, cx| {
            drop(store.run_operation(GitOperationKind::Push, cx));
            store
                .jobs
                .values()
                .find_map(|job| job.cancellation.clone())
                .expect("推送任务应持有取消句柄")
        });
        let cancel_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            cancellation.cancel();
        });
        cx.run_until_parked();
        cancel_thread.join().expect("取消线程不应异常");
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "运行中取消应及时结束"
        );
        assert!(
            cx.read_entity(&git_store, |store, _| store
                .jobs
                .values()
                .all(|job| job.operation.is_none())),
            "进程退出并确认状态后应释放远程操作闸门"
        );

        let rev = rev_parse;
        assert_ne!(rev(&remote), rev(&root), "取消后远端不应包含待推送提交");

        fs::remove_file(&hook_path).expect("应移除测试钩子");
        git_store.update(cx, |store, cx| {
            drop(store.run_operation(GitOperationKind::Push, cx));
        });
        cx.run_until_parked();
        assert_eq!(rev(&remote), rev(&root), "取消完成后再次推送应成功");
    }

    #[gpui::test]
    fn git_init_then_scan_discovers_repository(cx: &mut gpui::TestAppContext) {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let root = temp_dir.path().to_path_buf();

        let git_store = cx.new(|cx| GitStore::new(Some(root.clone()), cx));
        git_store.update(cx, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();
        let empty = cx.read_entity(&git_store, |store, _| !store.has_repositories());
        assert!(empty, "无仓库目录首次扫描后应无仓库");

        // git init → 后台 job 完成后触发重扫 → 新仓库被发现。
        git_store.update(cx, |store, cx| store.git_init(cx));
        cx.run_until_parked(); // init job 完成
        cx.run_until_parked(); // 其触发的全量重扫落地

        let ready = cx.read_entity(&git_store, |store, _| store.has_repositories());
        assert!(ready, "git init 后应发现新仓库");
        // init 后为空仓库（无提交），branch/head 按设计为 None，这里只验证仓库被发现。
        let count = cx.read_entity(&git_store, |store, _| store.repositories().count());
        assert_eq!(count, 1, "应恰好发现一个仓库");
    }

    #[gpui::test]
    fn stage_paths_moves_file_between_sections(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        fs::write(root.join("tracked.txt"), "修改后的内容\n").expect("应修改文件");

        let git_store = cx.new(|cx| GitStore::new(Some(root.clone()), cx));
        git_store.update(cx, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 修改文件初始为未暂存（index Unmodified、worktree Modified）。
        let unstaged = cx.read_entity(&git_store, |store, _| {
            matches!(
                store
                    .status_for_path(&root.join("tracked.txt"))
                    .map(|entry| entry.status),
                Some(FileStatus::Tracked {
                    index_status: StatusCode::Unmodified,
                    worktree_status: StatusCode::Modified
                })
            )
        });
        assert!(unstaged, "修改后应为未暂存状态");
        assert!(
            !cx.read_entity(&git_store, |store, _| store.has_staged_changes()),
            "只有未暂存改动时不应具备提交资格"
        );

        // 暂存 → 后台 job + 重扫 → index 变为 Modified。
        git_store.update(cx, |store, cx| {
            store.stage_paths(vec![root.join("tracked.txt")], cx);
        });
        cx.run_until_parked(); // stage job 完成
        cx.run_until_parked(); // 其触发的重扫落地
        let staged = cx.read_entity(&git_store, |store, _| {
            matches!(
                store
                    .status_for_path(&root.join("tracked.txt"))
                    .map(|entry| entry.status),
                Some(FileStatus::Tracked {
                    index_status: StatusCode::Modified,
                    worktree_status: StatusCode::Unmodified
                })
            )
        });
        assert!(staged, "暂存后 index 应为 Modified、worktree 干净");
        assert!(
            cx.read_entity(&git_store, |store, _| store.has_staged_changes()),
            "存在已暂存改动时应具备提交资格"
        );

        // 取消暂存 → 回到未暂存。
        git_store.update(cx, |store, cx| {
            store.unstage_paths(vec![root.join("tracked.txt")], cx);
        });
        cx.run_until_parked();
        cx.run_until_parked();
        let unstaged_again = cx.read_entity(&git_store, |store, _| {
            matches!(
                store
                    .status_for_path(&root.join("tracked.txt"))
                    .map(|entry| entry.status),
                Some(FileStatus::Tracked {
                    index_status: StatusCode::Unmodified,
                    worktree_status: StatusCode::Modified
                })
            )
        });
        assert!(unstaged_again, "取消暂存后应回到未暂存状态");
        assert!(
            !cx.read_entity(&git_store, |store, _| store.has_staged_changes()),
            "取消全部暂存后应失去提交资格"
        );
    }

    #[gpui::test]
    fn stage_paths_expands_directory_to_matching_files(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        // src 下的已跟踪修改 + 未跟踪新文件 + 子目录文件。
        std::fs::create_dir_all(root.join("src/sub")).expect("应创建目录");
        fs::write(root.join("src/a.txt"), "改动的 a\n").expect("应写入文件");
        fs::write(root.join("src/new.txt"), "新文件\n").expect("应写入文件");
        fs::write(root.join("src/sub/b.txt"), "改动的 b\n").expect("应写入文件");

        let git_store = cx.new(|cx| GitStore::new(Some(root.clone()), cx));
        git_store.update(cx, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 暂存整个 src 目录：修改 + 未跟踪 + 子目录文件一并进入 index。
        git_store.update(cx, |store, cx| {
            store.stage_paths(vec![root.join("src")], cx);
        });
        cx.run_until_parked();
        cx.run_until_parked();
        let staged = cx.read_entity(&git_store, |store, _| {
            ["src/a.txt", "src/new.txt", "src/sub/b.txt"]
                .into_iter()
                .all(|relative| {
                    store
                        .status_for_path(&root.join(relative))
                        .is_some_and(|entry| entry.status.has_staged())
                })
        });
        assert!(staged, "目录暂存后其下所有变更文件都应已暂存");

        // 取消暂存整个目录：全部回到未暂存（新文件回到未跟踪）。
        git_store.update(cx, |store, cx| {
            store.unstage_paths(vec![root.join("src")], cx);
        });
        cx.run_until_parked();
        cx.run_until_parked();
        let unstaged = cx.read_entity(&git_store, |store, _| {
            ["src/a.txt", "src/new.txt", "src/sub/b.txt"]
                .into_iter()
                .all(|relative| {
                    store
                        .status_for_path(&root.join(relative))
                        .is_some_and(|entry| entry.status.has_unstaged())
                })
        });
        assert!(unstaged, "目录取消暂存后其下所有文件都应回到未暂存");
    }

    #[gpui::test]
    fn request_hunks_fills_hunks_on_demand(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let tracked = canonicalize_path(&root.join("tracked.txt"));
        let request = DiffRequest::new(DiffBase::Head, tracked.clone());
        // 干净文件无需查询，直接得到空终态，也不会创建后台任务。
        cx.update_entity(&git_store, |store, cx| {
            store.set_hunk_interests(std::slice::from_ref(&request), cx)
        });
        cx.run_until_parked();
        cx.read_entity(&git_store, |store, _| {
            assert_eq!(
                store
                    .hunks_for_path(DiffBase::Head, &tracked)
                    .map(|hunks| hunks.len()),
                Some(0)
            );
            assert!(!store.pending_jobs.contains_key(&GitJobKey::RefreshHunks));
            assert!(matches!(
                store
                    .diff_coordinator
                    .records
                    .get(&request)
                    .map(|record| &record.state),
                Some(HunkState::NotNeeded)
            ));
        });

        // 外部修改第 2 行 → 按需请求 hunks。
        fs::write(&tracked, "第一行\n改了第二行\n").expect("应修改文件");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(std::slice::from_ref(&tracked), cx)
        });
        cx.run_until_parked();
        cx.update_entity(&git_store, |store, cx| {
            store.request_hunks(DiffBase::Head, std::slice::from_ref(&tracked), cx)
        });
        cx.run_until_parked();

        let hunks = cx
            .read_entity(&git_store, |store, _| {
                store.hunks_for_path(DiffBase::Head, &tracked)
            })
            .expect("请求后应有 hunks");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].range, 1..2);
        assert_eq!(hunks[0].kind, zcv_git::DiffHunkKind::Modified);
    }

    #[gpui::test]
    fn refreshed_path_creates_new_hunk_generation_even_when_status_kind_is_unchanged(
        cx: &mut gpui::TestAppContext,
    ) {
        let (root, _temp) = test_git_repo();
        let tracked = root.join("tracked.txt");
        fs::write(&tracked, "第一行\n第一次修改\n").expect("应修改文件");
        let tracked = canonicalize_path(&tracked);
        let request = DiffRequest::new(DiffBase::Head, tracked.clone());
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();
        cx.update_entity(&git_store, |store, cx| {
            store.set_hunk_interests(std::slice::from_ref(&request), cx)
        });
        cx.run_until_parked();
        let first_generation = cx.read_entity(&git_store, |store, _| {
            store.diff_coordinator.records[&request].generation
        });

        // 文件仍然是同一种已跟踪修改，行数统计也不变，但内容版本必须失效并重新查询。
        fs::write(&tracked, "第一行\n第二次修改\n").expect("应再次修改文件");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(std::slice::from_ref(&tracked), cx)
        });
        cx.run_until_parked();

        cx.read_entity(&git_store, |store, _| {
            let record = &store.diff_coordinator.records[&request];
            assert!(record.generation > first_generation);
            assert!(matches!(record.state, HunkState::Ready(_)));
        });
    }

    #[gpui::test]
    fn stale_hunk_result_cannot_overwrite_newer_generation(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let tracked = root.join("tracked.txt");
        fs::write(&tracked, "第一行\n已修改\n").expect("应修改文件");
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let request = DiffRequest::new(DiffBase::Head, tracked.clone());
        cx.update_entity(&git_store, |store, cx| {
            store.diff_coordinator.records.insert(
                request.clone(),
                HunkRecord {
                    generation: 2,
                    state: HunkState::Queued,
                },
            );
            store.diff_coordinator.in_flight.insert(request.clone(), 1);
            store.commit_job(
                &GitJob::RefreshHunks,
                JobResult::RefreshHunks(vec![(
                    0,
                    vec![(
                        DiffRequest::new(DiffBase::Head, PathBuf::from("tracked.txt")),
                        Ok(vec![zcv_git::DiffHunk {
                            range: 0..1,
                            old_range: 0..1,
                            kind: zcv_git::DiffHunkKind::Modified,
                        }]),
                    )],
                )]),
                cx,
            );
        });

        cx.read_entity(&git_store, |store, _| {
            let record = &store.diff_coordinator.records[&request];
            assert_eq!(record.generation, 2);
            assert!(matches!(record.state, HunkState::Queued));
        });
    }

    #[gpui::test]
    fn active_repository_follows_focused_path(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let nested = root.join("nested");
        fs::create_dir(&nested).expect("应创建嵌套目录");
        run_git(&nested, &["init", "-q", "-b", "feature"]);
        run_git(&nested, &["config", "user.email", "test@example.com"]);
        run_git(&nested, &["config", "user.name", "Test User"]);
        fs::write(nested.join("n.txt"), "嵌套\n").expect("应写入嵌套文件");
        run_git(&nested, &["add", "n.txt"]);
        run_git(&nested, &["commit", "-q", "-m", "nested initial"]);

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 初始 active = 第一个发现的仓库（root，分支 master）。
        let branch = |cx: &mut gpui::TestAppContext, store: &gpui::Entity<GitStore>| {
            cx.read_entity(store, |store, _| store.current_branch().map(str::to_string))
        };
        assert_eq!(branch(cx, &git_store).as_deref(), Some("master"));

        // 焦点切到嵌套仓库内文件 → active 跟随，分支变 feature。
        cx.update_entity(&git_store, |store, cx| {
            store.set_active_repository_for_path(&nested.join("n.txt"), cx);
        });
        assert_eq!(branch(cx, &git_store).as_deref(), Some("feature"));

        // 焦点切回 root 仓库文件 → 回到 master。
        cx.update_entity(&git_store, |store, cx| {
            store.set_active_repository_for_path(&root.join("tracked.txt"), cx);
        });
        assert_eq!(branch(cx, &git_store).as_deref(), Some("master"));

        // 不在任何仓库中的路径（如已删除文件）→ active 保持不变。
        cx.update_entity(&git_store, |store, cx| {
            store.set_active_repository_for_path(&root.join(".."), cx);
        });
        assert_eq!(branch(cx, &git_store).as_deref(), Some("master"));
    }

    #[gpui::test]
    fn git_operation_targets_active_repository(cx: &mut gpui::TestAppContext) {
        // 根仓库无 remote；嵌套仓库有 remote。active 切到嵌套后 push 应作用于嵌套。
        let (root, _temp) = test_git_repo();
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let remote = temp_dir.path().join("remote.git");
        run_git(
            temp_dir.path(),
            &["init", "-q", "--bare", remote.to_str().unwrap()],
        );
        let nested = root.join("nested");
        fs::create_dir(&nested).expect("应创建嵌套目录");
        run_git(&nested, &["init", "-q", "-b", "master"]);
        run_git(&nested, &["config", "user.email", "test@example.com"]);
        run_git(&nested, &["config", "user.name", "Test User"]);
        fs::write(nested.join("n.txt"), "嵌套\n").expect("应写入嵌套文件");
        run_git(&nested, &["add", "n.txt"]);
        run_git(&nested, &["commit", "-q", "-m", "nested initial"]);
        run_git(
            &nested,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_git(&nested, &["push", "-q", "-u", "origin", "master"]);

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // active 切到嵌套仓库，本地新提交后 push。
        cx.update_entity(&git_store, |store, cx| {
            store.set_active_repository_for_path(&nested.join("n.txt"), cx);
        });
        fs::write(nested.join("new.txt"), "新提交\n").expect("应写入文件");
        run_git(&nested, &["add", "new.txt"]);
        run_git(&nested, &["commit", "-q", "-m", "新提交"]);
        cx.update_entity(&git_store, |store, cx| {
            drop(store.run_operation(GitOperationKind::Push, cx));
        });
        cx.run_until_parked();
        let job_done = cx.read_entity(&git_store, |store, _| {
            !store
                .pending_jobs
                .contains_key(&GitJobKey::GitOperation(GitOperationKind::Push))
        });
        assert!(job_done, "push job 应已完成");

        // 远端应指向嵌套仓库 HEAD（而非根仓库）。
        let rev = rev_parse;
        assert_eq!(rev(&remote), rev(&nested), "push 应作用于活动仓库（嵌套）");
    }

    #[gpui::test]
    fn initial_scan_sets_active_to_first_repository(cx: &mut gpui::TestAppContext) {
        // root 位于外层仓库内且包含嵌套仓库：祖先前置 → 初始 active = 外层仓库。
        let (outer, _temp) = test_git_repo();
        let root = outer.join("proj");
        fs::create_dir(&root).expect("应创建项目目录");
        let nested = root.join("nested");
        fs::create_dir(&nested).expect("应创建嵌套目录");
        run_git(&nested, &["init", "-q", "-b", "feature"]);
        run_git(&nested, &["config", "user.email", "test@example.com"]);
        run_git(&nested, &["config", "user.name", "Test User"]);
        fs::write(nested.join("n.txt"), "嵌套\n").expect("应写入嵌套文件");
        run_git(&nested, &["add", "n.txt"]);
        run_git(&nested, &["commit", "-q", "-m", "nested initial"]);

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let branch = cx.read_entity(&git_store, |store, _| {
            store.current_branch().map(str::to_string)
        });
        assert_eq!(
            branch.as_deref(),
            Some("master"),
            "初始 active 应为外层仓库"
        );
    }

    #[gpui::test]
    fn remote_operation_state_reflects_push(cx: &mut gpui::TestAppContext) {
        // 工作仓库与裸远程共用 temp_dir，保证测试期间目录存活。
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let remote = temp_dir.path().join("remote.git");
        run_git(
            temp_dir.path(),
            &["init", "-q", "--bare", remote.to_str().unwrap()],
        );
        let root = temp_dir.path().join("work");
        fs::create_dir(&root).expect("应创建工作仓库目录");
        run_git(&root, &["init", "-q", "-b", "master"]);
        run_git(&root, &["config", "user.email", "test@example.com"]);
        run_git(&root, &["config", "user.name", "Test User"]);
        fs::write(root.join("tracked.txt"), "内容\n").expect("应写入初始文件");
        run_git(&root, &["add", "tracked.txt"]);
        run_git(&root, &["commit", "-q", "-m", "initial"]);
        run_git(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_git(&root, &["push", "-q", "-u", "origin", "master"]);

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 与远程同步：有 remote，无 ahead/behind。
        let state = cx.read_entity(&git_store, |store, _| store.remote_operation_state());
        assert_eq!(
            state,
            RemoteOperationState {
                has_remote: true,
                ahead: 0,
                behind: 0,
                ..Default::default()
            }
        );

        // 本地新提交 → ahead 1（可推送数）。
        fs::write(root.join("new.txt"), "新提交\n").expect("应写入文件");
        run_git(&root, &["add", "new.txt"]);
        run_git(&root, &["commit", "-q", "-m", "本地提交"]);
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();
        let state = cx.read_entity(&git_store, |store, _| store.remote_operation_state());
        assert_eq!(state.ahead, 1);

        // push 后回到同步（徽标消失链路：ahead 变化必须触发事件）。
        cx.update_entity(&git_store, |store, cx| {
            drop(store.run_operation(GitOperationKind::Push, cx));
        });
        cx.run_until_parked();
        cx.run_until_parked(); // 等 push 完成后触发的重新扫描落地。
        let state = cx.read_entity(&git_store, |store, _| store.remote_operation_state());
        assert_eq!(
            state,
            RemoteOperationState {
                has_remote: true,
                ahead: 0,
                behind: 0,
                ..Default::default()
            },
            "push 后 ahead 应归零"
        );
    }

    #[gpui::test]
    fn remote_operation_state_defaults_without_remote(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let state = cx.read_entity(&git_store, |store, _| store.remote_operation_state());
        assert_eq!(state, RemoteOperationState::default());
    }

    #[gpui::test]
    fn request_hunks_skips_untracked_files(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 未跟踪文件进入 NotNeeded 终态，不创建查询任务。
        let untracked = root.join("untracked.txt");
        fs::write(&untracked, "新的\n").expect("应写入文件");
        let untracked = canonicalize_path(&untracked);
        let request = DiffRequest::new(DiffBase::Head, untracked.clone());
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(std::slice::from_ref(&untracked), cx)
        });
        cx.run_until_parked();
        cx.update_entity(&git_store, |store, cx| {
            store.request_hunks(DiffBase::Head, std::slice::from_ref(&untracked), cx)
        });
        cx.run_until_parked();

        cx.read_entity(&git_store, |store, _| {
            assert_eq!(
                store
                    .hunks_for_path(DiffBase::Head, &untracked)
                    .map(|hunks| hunks.len()),
                Some(0)
            );
            assert!(!store.pending_jobs.contains_key(&GitJobKey::RefreshHunks));
            assert!(matches!(
                store
                    .diff_coordinator
                    .records
                    .get(&request)
                    .map(|record| &record.state),
                Some(HunkState::NotNeeded)
            ));
        });
    }

    #[gpui::test]
    fn scan_reports_branch_list(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        run_git(&root, &["checkout", "-q", "-b", "feature"]);

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let branches = cx.read_entity(&git_store, |store, _| {
            store.active_branch_list().map(|branches| branches.to_vec())
        });
        let branches = branches.expect("应有分支列表");
        let by_name: HashMap<_, _> = branches
            .iter()
            .map(|branch| (branch.name.as_str(), branch.is_head))
            .collect();
        assert_eq!(by_name.get("master"), Some(&false));
        assert_eq!(by_name.get("feature"), Some(&true));
    }

    #[gpui::test]
    fn checkout_branch_switches_and_refreshes(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        run_git(&root, &["checkout", "-q", "-b", "feature"]);

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 选择器确认切换到 master：job 完成后自动重扫，Head 事件驱动 UI 刷新。
        cx.update_entity(&git_store, |store, cx| {
            store.checkout_branch("master".into(), cx);
        });
        cx.run_until_parked();
        cx.run_until_parked(); // 等 checkout 完成后触发的重新扫描落地。

        let (branch, is_master_head) = cx.read_entity(&git_store, |store, _| {
            let branch = store.current_branch().map(str::to_string);
            let is_master_head = store.active_branch_list().is_some_and(|branches| {
                branches
                    .iter()
                    .find(|branch| branch.name == "master")
                    .is_some_and(|branch| branch.is_head)
            });
            (branch, is_master_head)
        });
        assert_eq!(branch.as_deref(), Some("master"));
        assert!(is_master_head);
    }

    #[gpui::test]
    fn create_branch_creates_and_refreshes(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 选择器"创建分支"行确认：以当前 HEAD 为基创建并切换。
        cx.update_entity(&git_store, |store, cx| {
            store.create_branch("new-branch".into(), cx);
        });
        cx.run_until_parked();
        cx.run_until_parked();

        let (branch, has_new) = cx.read_entity(&git_store, |store, _| {
            let branch = store.current_branch().map(str::to_string);
            let has_new = store
                .active_branch_list()
                .is_some_and(|branches| branches.iter().any(|branch| branch.name == "new-branch"));
            (branch, has_new)
        });
        assert_eq!(branch.as_deref(), Some("new-branch"));
        assert!(has_new);
    }

    #[gpui::test]
    fn external_checkout_updates_branch_list(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        run_git(&root, &["checkout", "-q", "-b", "feature"]);

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 外部 checkout 回 master：增量刷新含 .git 路径 → 分支列表随 head 重读。
        run_git(&root, &["checkout", "-q", "master"]);
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(&[root.join(".git/HEAD")], cx)
        });
        cx.run_until_parked();

        let is_master_head = cx.read_entity(&git_store, |store, _| {
            store.active_branch_list().is_some_and(|branches| {
                branches
                    .iter()
                    .find(|branch| branch.name == "master")
                    .is_some_and(|branch| branch.is_head)
            })
        });
        assert!(is_master_head);
    }

    #[gpui::test]
    fn branch_ops_skip_when_no_repository(cx: &mut gpui::TestAppContext) {
        // 非 git 目录：checkout/create 入口不 panic，仅触发扫描后返回。
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let git_store =
            cx.update(|cx| cx.new(|cx| GitStore::new(Some(temp_dir.path().to_path_buf()), cx)));
        cx.update_entity(&git_store, |store, cx| {
            store.checkout_branch("master".into(), cx);
            store.create_branch("feature".into(), cx);
        });
        cx.run_until_parked();
    }
}
