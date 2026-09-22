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
mod jobs;
mod snapshots;

use jobs::{GitJob, GitJobId, GitJobKey, GitJobRecord, ScheduledGitJob};
pub use jobs::{GitJobPhase, GitJobStatus, GitOperationKind};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use background::{JobResult, execute_job};
use gpui::{
    App, AppContext as _, AsyncApp, BackgroundExecutor, Context, Entity, EventEmitter, Task,
    WeakEntity,
};
use zcv_buffer_diff::{BufferDiff, BufferDiffInput, DiffOperations, PendingHunk};
use zcv_git::{
    Branch, DiffStat, FileStatus, GitCancellation, GitHunkOperation, GitRepository, GitRevision,
    GraphCommit, HunkEdit, WorkingCopySnapshot, apply_hunk_edits_to_text,
};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_path::{AbsolutePathBuf, RelativePathBuf, normalize_for_comparison};
use zcv_text::{Anchor, Buffer, BufferConfig, ByteOffset, Snapshot, TextRange};

/// 一次增量刷新最多累积的路径数，超过则升级为全量扫描。
const MAX_INCREMENTAL_PATHS: usize = 500;

/// GitStore 通知事件。
///
/// 单窗口简化：事件均无 payload（除提交撤销结果），订阅方收到后按需重读 GitStore 状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitStoreEvent {
    /// 仓库集合发生变化（发现/消失）。
    Repositories,
    /// 文件状态或 diff 统计发生变化。
    Statuses,
    /// 指定路径的 index 文本已在内存中乐观更新或回滚；
    /// 订阅方只需重读该路径的 `GitRevision::Index` 并重挂该路径的 diff。
    IndexText { path: AbsolutePathBuf },
    /// 当前分支、HEAD 或分支列表发生变化。
    Head,
    /// 活动仓库变化（跟随焦点文件切换；订阅方重读 `current_branch()`，无需 payload）。
    ActiveRepositoryChanged,
    /// 后台 job 集合变化（开始/完成/取消）；订阅方重读 `current_job()`。
    JobsUpdated,
    /// 撤销提交成功：携带被撤销的提交消息（面板填回提交信息编辑器）。
    Uncommitted(String),
    /// 撤销提交失败：携带完整错误信息，由工作区向用户提示。
    UncommitFailed(String),
    /// 变更块操作失败：携带错误信息（面板提示错误并恢复被 optimistic 抑制的 hunk）。
    HunkOperationFailed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitOperationOutcome {
    Completed,
    Cancelled,
    CompletedBeforeCancellation,
    CancellationUnconfirmed(String),
    Failed(String),
}

fn operation_result_sender(job: &GitJob) -> Option<async_channel::Sender<GitOperationOutcome>> {
    match job {
        GitJob::GitOperation { on_done, .. }
        | GitJob::CheckoutBranch { on_done, .. }
        | GitJob::CreateBranch { on_done, .. }
        | GitJob::DeleteBranch { on_done, .. } => on_done.clone(),
        _ => None,
    }
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
    pub statuses_by_path: BTreeMap<RelativePathBuf, StatusEntry>,
}

/// 可脱离 GPUI 的 Git 状态只读快照，供项目树后台计算可见行状态。
///
/// GitStore 仍是唯一状态所有者；该类型只是一次不可变的派生视图，不参与写回。
#[derive(Clone, Default)]
pub(crate) struct GitStatusSnapshot {
    repositories: Vec<GitRepositoryStatusSnapshot>,
}

#[derive(Clone)]
struct GitRepositoryStatusSnapshot {
    working_directory: AbsolutePathBuf,
    statuses_by_path: BTreeMap<RelativePathBuf, StatusEntry>,
    directory_statuses: BTreeMap<RelativePathBuf, FileStatus>,
}

fn directory_statuses(
    statuses: &BTreeMap<RelativePathBuf, StatusEntry>,
) -> BTreeMap<RelativePathBuf, FileStatus> {
    let mut directories = BTreeMap::new();
    for (path, entry) in statuses {
        if entry.status.is_ignored() {
            continue;
        }
        let mut parent = path
            .as_path()
            .parent()
            .map(|parent| RelativePathBuf::from_path(parent).expect("Git 相对路径必须有效"));
        while let Some(directory) = parent.as_ref().filter(|directory| !directory.is_empty()) {
            let directory = directory.clone();
            directories
                .entry(directory.clone())
                .and_modify(|current: &mut FileStatus| {
                    if entry.status.priority() > current.priority() {
                        *current = entry.status;
                    }
                })
                .or_insert(entry.status);
            parent = directory
                .as_path()
                .parent()
                .map(|parent| RelativePathBuf::from_path(parent).expect("Git 相对路径必须有效"));
        }
    }
    directories
}

impl GitStatusSnapshot {
    pub(crate) fn statuses_for_rows(
        &self,
        rows: &[(AbsolutePathBuf, bool)],
    ) -> HashMap<AbsolutePathBuf, FileStatus> {
        rows.iter()
            .filter_map(|(path, is_dir)| {
                // 项目树传入的路径已经由 Worktree 统一规范化；这里直接以语义路径查询快照。
                let canonical_path = path;
                let status = if *is_dir {
                    self.status_for_directory(canonical_path.as_path())
                } else {
                    self.status_for_path(canonical_path.as_path())
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
        let relative = repository.working_directory.relative_path(path)?;
        repository
            .statuses_by_path
            .get(&relative)
            .or_else(|| GitStore::ignored_ancestor_entry(&repository.statuses_by_path, &relative))
    }

    fn status_for_directory(&self, path: &Path) -> Option<FileStatus> {
        let repository = self.repository_for_path(path)?;
        let relative = repository.working_directory.relative_path(path)?;
        let statuses = &repository.statuses_by_path;
        if let Some(entry) = statuses.get(&relative)
            && entry.status.is_ignored()
        {
            return Some(FileStatus::Ignored);
        }
        repository
            .directory_statuses
            .get(&relative)
            .copied()
            .or_else(|| {
                GitStore::ignored_ancestor_entry(statuses, &relative).map(|entry| entry.status)
            })
    }
}

pub(super) struct Repository {
    repository: Arc<dyn GitRepository>,
    snapshot: RepositorySnapshot,
}

/// 读取已由仓库发现阶段确认的工作目录路径。
///
/// 仓库身份不应在快照合并或状态索引重建时重新访问文件系统；
/// 仓库目录可能正在被删除，但此前已经确认的绝对路径仍然是这条仓库记录的有效身份。
fn repository_working_directory(repository: &dyn GitRepository) -> AbsolutePathBuf {
    AbsolutePathBuf::new(repository.working_directory().to_path_buf())
        .expect("Git 仓库工作目录必须是绝对路径")
}

/// 共享 diff 缓存的键：路径、working 实体、base 文档、index 文档。
type SharedDiffKey = (
    AbsolutePathBuf,
    gpui::EntityId,
    Option<gpui::EntityId>,
    Option<gpui::EntityId>,
);

pub struct GitStore {
    /// 项目根目录；无 worktree 的空项目为 None，此时所有 job 与仓库查询为空操作。
    root: Option<AbsolutePathBuf>,
    repositories: Vec<Repository>,
    /// 是否已完成至少一次仓库发现；空集合也表示扫描已完成。
    repository_scan_ready: bool,
    /// 当前仓库状态的不可变派生索引；
    /// 项目树只克隆 Arc，不在 UI 线程复制状态表。
    status_index: Arc<GitStatusSnapshot>,
    /// 活动仓库（按 working_directory 标识）：分支显示与 fetch/pull/push 等 git 操作的目标。
    /// 用 working_directory 而非索引：全量扫描重建 Vec，索引不稳定。
    active_repo_workdir: Option<AbsolutePathBuf>,
    /// HEAD/index 修订文档缓存；状态或 HEAD 变化时失效。
    /// 值 `None` 表示该修订中文件不存在（已加载但缺失），键存在即表示已加载完成。
    /// 修订文档是 HEAD/index 的唯一权威实例，工作区视图与 diff 都从这里取用。
    revision_documents: HashMap<(GitRevision, AbsolutePathBuf), Option<Entity<LanguageBuffer>>>,
    /// 分修订递增的缓存版本；失效前启动的后台读取不得回填新缓存。
    revision_generations: HashMap<GitRevision, u64>,
    /// 已写入内存、尚待后台落盘确认的 index 文本的原始值；同一路径同时只允许一个写入，失败时据此回滚。
    optimistic_index_bases: HashMap<AbsolutePathBuf, Arc<str>>,
    /// 按 (路径, working 实体, base 文档, index 文档) 共享的 diff 实体；
    /// 同一份 diff 跨编辑器 / 面板视图复用，head/index 变化时按路径失效。
    shared_diffs: HashMap<SharedDiffKey, Entity<BufferDiff>>,
    /// 项目唯一的语言注册表；修订文档与工作区文档共用。
    language_registry: Arc<LanguageRegistry>,
    background: BackgroundExecutor,
    /// 自身弱句柄：后台任务完成后回填缓存等状态用（构造时注入）。
    self_handle: WeakEntity<Self>,
    job_sender: async_channel::Sender<ScheduledGitJob>,
    next_job_id: GitJobId,
    pending_jobs: HashMap<GitJobKey, GitJobId>,
    jobs: HashMap<GitJobId, GitJobRecord>,
    in_flight: Option<GitJobId>,
    paths_needing_status_update: BTreeSet<AbsolutePathBuf>,
    _job_task: Task<()>,
}

impl GitStore {
    pub fn new(
        root: Option<PathBuf>,
        language_registry: Arc<LanguageRegistry>,
        cx: &mut Context<Self>,
    ) -> Self {
        // 仓库的 working_directory 来自 canonicalize，root 同样归一化，保证路径前缀匹配一致。
        let root = root
            .map(|root| canonicalize_path(&root))
            .transpose()
            .expect("项目根路径必须可归一化");
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
                        if let Some(tx) = operation_result_sender(&job) {
                            let _ = tx.send(GitOperationOutcome::Cancelled).await;
                        }
                        this.update(&mut cx, |store, cx| store.finish_job(scheduled.id, cx));
                        continue;
                    }
                    let Some(prepared) = this.update(&mut cx, |store, _| store.prepare_job(&job))
                    else {
                        this.update(&mut cx, |store, cx| store.finish_job(scheduled.id, cx));
                        continue;
                    };
                    // 标记在途任务（状态栏开始显示）。
                    this.update(&mut cx, |store, cx| store.set_in_flight(scheduled.id, cx));
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
                        this.update(&mut cx, |store, cx| {
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
                    this.update(&mut cx, |store, cx| store.clear_in_flight(scheduled.id, cx));
                    // 操作结果回传发起方（Workspace await 后直接弹提示）。
                    if let Some(tx) = operation_result_sender(&job) {
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
                    this.update(&mut cx, |store, cx| {
                        // 不合并已取消命令的部分结果，后续确认与全量扫描才是状态真值来源。
                        if cancelled {
                            store.schedule_scan(cx);
                        } else {
                            store.commit_job(&job, result, cx);
                        }
                        store.finish_job(scheduled.id, cx);
                        store.schedule_pending_status_refresh(cx);
                    });
                }
            }
        });

        let self_handle = cx.weak_entity();
        Self {
            root,
            repositories: Vec::new(),
            repository_scan_ready: false,
            status_index: Arc::new(GitStatusSnapshot::default()),
            active_repo_workdir: None,
            revision_documents: HashMap::new(),
            revision_generations: HashMap::from([(GitRevision::Head, 1), (GitRevision::Index, 1)]),
            optimistic_index_bases: HashMap::new(),
            shared_diffs: HashMap::new(),
            language_registry,
            background,
            self_handle,
            job_sender,
            next_job_id: 1,
            pending_jobs: HashMap::new(),
            jobs: HashMap::new(),
            in_flight: None,
            paths_needing_status_update: BTreeSet::new(),
            _job_task: job_task,
        }
    }

    /// 全量扫描：重新发现仓库并重扫所有状态（初始扫描与结构性变化时调用）。
    pub(super) fn schedule_scan(&mut self, cx: &mut Context<Self>) {
        self.paths_needing_status_update.clear();
        self.schedule_job(GitJob::ReloadGitState, cx);
    }

    /// 当前任务结束后补发执行期间累积的增量路径。
    ///
    /// 文件监听事件可能在刷新任务执行期间到达；
    /// 任务去重会跳过重复排队，因此必须在原任务完成后显式消费这批路径。
    fn schedule_pending_status_refresh(&mut self, cx: &mut Context<Self>) {
        if self.paths_needing_status_update.is_empty()
            || self.pending_jobs.contains_key(&GitJobKey::ReloadGitState)
            || self.pending_jobs.contains_key(&GitJobKey::RefreshStatuses)
        {
            return;
        }
        self.schedule_job(GitJob::RefreshStatuses, cx);
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
    pub fn stage_paths(&mut self, paths: Vec<AbsolutePathBuf>, cx: &mut Context<Self>) {
        self.schedule_job(GitJob::StageFiles { stage: true, paths }, cx);
    }

    /// 取消暂存路径（面板复选框取消勾选触发；`git reset`），完成后自动重新扫描。
    pub fn unstage_paths(&mut self, paths: Vec<AbsolutePathBuf>, cx: &mut Context<Self>) {
        self.schedule_job(
            GitJob::StageFiles {
                stage: false,
                paths,
            },
            cx,
        );
    }

    /// 清除已解决文件的冲突 stage，并以当前分支内容作为未暂存基线。
    pub fn resolve_conflicts(&mut self, paths: Vec<AbsolutePathBuf>, cx: &mut Context<Self>) {
        self.schedule_job(GitJob::ResolveConflicts { paths }, cx);
    }

    /// 构造变更块操作实现（宿主注入 `BufferDiff` 时使用）。
    ///
    /// `base` 决定该 diff 支持的操作方向：index 为基（未暂存差异）可暂存/还原，HEAD 为基（已暂存差异）可取消暂存。
    pub fn diff_operations(&self, base: GitRevision) -> Arc<dyn DiffOperations> {
        Arc::new(GitDiffOperations {
            store: self.self_handle.clone(),
            base,
        })
    }

    /// 按 (路径, working 实体, base 文档, index 文档) 共享单个文件的 diff 实体。
    ///
    /// 同一份 diff 跨编辑器与面板视图复用；
    /// head/index 文档变化时由失效逻辑丢弃缓存，下一次请求会用新文档重建实体。
    pub fn file_diff(
        &mut self,
        input: &BufferDiffInput,
        cx: &mut Context<Self>,
    ) -> Entity<BufferDiff> {
        let path = canonicalize_path(&input.path).expect("diff 输入路径必须可归一化");
        let key = (
            path,
            input.working.entity_id(),
            input.base.as_ref().map(Entity::entity_id),
            input.index.as_ref().map(Entity::entity_id),
        );
        if let Some(entity) = self.shared_diffs.get(&key) {
            return entity.clone();
        }
        let entity = cx.new(|cx| BufferDiff::new(input.clone(), cx));
        self.shared_diffs.insert(key, entity.clone());
        entity
    }

    /// 丢弃共享 diff 缓存：None 清空全部，Some 只清指定路径。
    fn invalidate_shared_diffs(&mut self, paths: Option<&[AbsolutePathBuf]>) {
        match paths {
            None => self.shared_diffs.clear(),
            Some(paths) => {
                self.shared_diffs
                    .retain(|key, _| !paths.iter().any(|path| &key.0 == path));
            }
        }
    }

    /// 基于当前 diff 快照生成确定的编辑，先写入 optimistic pending，再交后台执行。
    ///
    /// 后台只把已经确定的字节编辑应用到 index 或工作区文本，不再重新执行磁盘 diff 定位变更块。
    fn apply_hunk_edits(
        &mut self,
        operation: GitHunkOperation,
        diff: Entity<BufferDiff>,
        ranges: Vec<Range<Anchor>>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let (path, edits, pending, working_snapshot, index_text) = {
            let diff_ref = diff.read(cx);
            let working = diff_ref.working().clone();
            let working_text = working.read(cx).text_snapshot();
            let base_text = diff_ref
                .base_source()
                .map(|base| Arc::<str>::from(snapshot_text(&base.read(cx).text_snapshot())));
            let mut edits = Vec::new();
            let mut pending = Vec::new();
            for range in &ranges {
                // 操作范围必须绑定当前工作区版本，旧版本锚点不得修改新版本 buffer。
                if range.start.version() != working_text.version()
                    || range.end.version() != working_text.version()
                {
                    continue;
                }
                for hunk in diff_ref.snapshot().hunks().iter().filter(|hunk| {
                    hunk.buffer_range.start.offset() <= range.end.offset()
                        && range.start.offset() <= hunk.buffer_range.end.offset()
                }) {
                    let working_range = hunk.buffer_range.start.offset().get()
                        ..hunk.buffer_range.end.offset().get();
                    let working_slice = working_text
                        .slice_text(
                            TextRange::new(
                                hunk.buffer_range.start.offset(),
                                hunk.buffer_range.end.offset(),
                            )
                            .expect("hunk 新侧范围必须有序"),
                        )
                        .expect("hunk 新侧范围必须有效")
                        .as_str()
                        .to_owned();
                    let base_slice = base_text
                        .as_deref()
                        .and_then(|base| base.get(hunk.diff_base_byte_range.clone()))
                        .unwrap_or_default()
                        .to_owned();
                    let (range, original, replacement) = match operation {
                        GitHunkOperation::Stage => {
                            (hunk.diff_base_byte_range.clone(), base_slice, working_slice)
                        }
                        GitHunkOperation::Unstage | GitHunkOperation::Restore => {
                            (working_range, working_slice, base_slice)
                        }
                    };
                    edits.push(HunkEdit::new(
                        range,
                        Arc::from(original),
                        Arc::from(replacement),
                    ));
                    pending.push(PendingHunk::suppress(hunk, working_text.version()));
                }
            }
            let working_snapshot = working_text
                .slice_text(
                    TextRange::new(ByteOffset::ZERO, working_text.len_bytes())
                        .expect("工作区全文范围必须有序"),
                )
                .expect("工作区全文范围必须有效")
                .as_str()
                .to_owned();
            let index_text = match operation {
                GitHunkOperation::Stage => base_text,
                GitHunkOperation::Unstage => Some(Arc::from(working_snapshot.as_str())),
                GitHunkOperation::Restore => None,
            };
            (
                diff_ref.path().clone(),
                edits,
                pending,
                working_snapshot.into_bytes(),
                index_text,
            )
        };
        if edits.is_empty() {
            return Err("变更块已过期，请刷新后重试".into());
        }
        let path = canonicalize_path(&path).map_err(|error| format!("路径归一化失败：{error}"))?;
        if let Some(index_text) = &index_text
            && self
                .revision_document_text(GitRevision::Index, &path, cx)
                .as_deref()
                != Some(index_text)
        {
            return Err("暂存区内容已变化，请刷新后重试".into());
        }
        // index 编辑以当前缓存文本为基准；
        // 同一路径的上一笔写入未确认前不再接受新 hunk，否则失败回滚会让后续编辑失去确定的基准文本。
        if self.optimistic_index_bases.contains_key(&path) {
            return Err("该文件的上一项变更块操作尚未完成".into());
        }
        let next_index_text = match index_text
            .as_deref()
            .map(|index_text| apply_hunk_edits_to_text(index_text, &edits))
            .transpose()
        {
            Ok(text) => text,
            // `edits` 来源于同一 BufferDiff 快照；
            // 若此处不再匹配，说明 index 缓存与快照已经分叉。
            // 不向后台提交不确定写入，后续状态刷新会重新建立权威 diff。
            Err(_) => return Err("变更块已被其他编辑改动，请刷新后重试".into()),
        };
        if matches!(
            operation,
            GitHunkOperation::Stage | GitHunkOperation::Unstage
        ) && next_index_text.is_none()
        {
            return Err("当前操作无法生成有效的暂存区内容".into());
        }
        let next_index_text = next_index_text.map(Arc::<str>::from);
        if let (Some(index_text), Some(next_index_text)) = (&index_text, &next_index_text) {
            self.optimistic_index_bases
                .insert(path.clone(), index_text.clone());
            self.update_revision_document_text(GitRevision::Index, &path, next_index_text, cx);
            // 乐观 index 更新：本路径的共享 diff 立即失效，视图按 IndexText 事件重新请求。
            self.invalidate_shared_diffs(Some(std::slice::from_ref(&path)));
            cx.emit(GitStoreEvent::IndexText { path: path.clone() });
        }
        diff.update(cx, |diff, cx| diff.set_pending_hunks(pending, cx));
        self.schedule_job(
            GitJob::ApplyHunkEdits {
                operation,
                path,
                edits,
                next_index_text,
                working_snapshot: WorkingCopySnapshot::from_editor_text(working_snapshot),
                diff,
            },
            cx,
        );
        Ok(())
    }

    /// 提交暂存内容（消息来自面板提交信息编辑器）。
    ///
    /// 成功后重扫，Head/Statuses 事件驱动面板清空编辑器并刷新上次提交信息。
    pub fn commit(&mut self, message: String, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            self.schedule_scan(cx);
            return;
        }
        if !self.has_staged_changes() {
            return;
        }
        self.schedule_job(GitJob::Commit { message }, cx);
    }

    /// 撤销最近一次提交；
    /// 根提交删除当前本地分支引用，普通提交回退到第一个父提交。
    /// 被撤销消息填回提交信息编辑器，失败由订阅方提示用户。
    pub fn uncommit(&mut self, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            self.schedule_scan(cx);
            return;
        }
        self.schedule_job(GitJob::Uncommit, cx);
    }

    pub fn checkout_branch_with_result(
        &mut self,
        name: String,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<GitOperationOutcome>> {
        if self.repositories.is_empty() {
            self.schedule_scan(cx);
            return Task::ready(Err(anyhow::anyhow!("git 仓库尚未就绪")));
        }
        let (result_tx, result_rx) = async_channel::unbounded();
        self.schedule_job(
            GitJob::CheckoutBranch {
                name,
                on_done: Some(result_tx),
            },
            cx,
        );
        cx.spawn(|_this: WeakEntity<Self>, _cx: &mut AsyncApp| async move {
            result_rx
                .recv()
                .await
                .map_err(|_| anyhow::anyhow!("分支切换结果通道已关闭"))
        })
    }

    /// 以当前 HEAD 为基创建并切换分支（选择器"创建分支"行触发），完成后自动重扫。
    pub fn create_branch(&mut self, name: String, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            self.schedule_scan(cx);
            return;
        }
        self.schedule_job(
            GitJob::CreateBranch {
                name,
                on_done: None,
            },
            cx,
        );
    }

    /// 删除指定本地分支，完成后自动重扫。
    pub fn delete_branch(&mut self, name: String, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            self.schedule_scan(cx);
            return;
        }
        self.schedule_job(
            GitJob::DeleteBranch {
                name,
                on_done: None,
            },
            cx,
        );
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
    pub(crate) fn status_snapshot(&self) -> Arc<GitStatusSnapshot> {
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
                        working_directory: repository_working_directory(
                            repository.repository.as_ref(),
                        ),
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
        let paths: BTreeSet<AbsolutePathBuf> = paths
            .iter()
            .filter_map(|path| canonicalize_path(path).ok())
            .filter(|path| {
                self.root
                    .as_ref()
                    .is_some_and(|root| path.starts_with(root.as_path()))
            })
            .map(|path| {
                // `.git` 元数据的变化会影响整个仓库的状态；
                // 将其归一化为仓库根，让后台以空 pathspec 查询完整工作树，而不是查询 `.git/index` 本身。
                let Some(repository) = self.repo_for_path(path.as_path()) else {
                    return path;
                };
                let relative = repository_working_directory(repository.repository.as_ref())
                    .relative_path(path.as_path());
                if relative.is_some_and(|relative| {
                    relative
                        .as_path()
                        .components()
                        .next()
                        .is_some_and(|component| component.as_os_str() == ".git")
                }) {
                    repository_working_directory(repository.repository.as_ref())
                } else {
                    path
                }
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

    /// 查询路径的当前 Git 状态；状态表是 GitStore 的不可变权威快照。
    ///
    /// 状态索引内部按仓库工作目录（canonicalize 后）比较，调用方传入的路径可能未归一化。
    pub fn status_for_path(&self, path: &Path) -> Option<&StatusEntry> {
        let path = canonicalize_path(path).ok()?;
        self.status_index.status_for_path(path.as_path())
    }

    /// 查找最近一个被忽略的祖先目录条目；自身无条目时用于继承忽略状态。
    ///
    /// 只认 Ignored 条目：祖先链上命中的首个目录条目若不是忽略（例如子树内被负向规则放行的路径，git 会为相关路径生成条目），不向下继承。
    fn ignored_ancestor_entry<'a>(
        statuses: &'a BTreeMap<RelativePathBuf, StatusEntry>,
        relative: &RelativePathBuf,
    ) -> Option<&'a StatusEntry> {
        let mut ancestor = relative
            .as_path()
            .parent()
            .map(|parent| RelativePathBuf::from_path(parent).expect("Git 相对路径必须有效"));
        while let Some(dir) = ancestor {
            if let Some(entry) = statuses.get(&dir)
                && entry.status.is_ignored()
            {
                return Some(entry);
            }
            ancestor = dir
                .as_path()
                .parent()
                .map(|parent| RelativePathBuf::from_path(parent).expect("Git 相对路径必须有效"));
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
        let Ok(path) = canonicalize_path(path) else {
            return;
        };
        let Some(repository) = self.repo_for_path(&path) else {
            return;
        };
        let workdir = repository_working_directory(repository.repository.as_ref());
        if self.active_repo_workdir.as_ref() != Some(&workdir) {
            self.active_repo_workdir = Some(workdir);
            cx.emit(GitStoreEvent::ActiveRepositoryChanged);
        }
    }

    /// 是否已发现至少一个 git 仓库（决定 git 相关 UI 是否可见）。
    pub fn has_repositories(&self) -> bool {
        !self.repositories.is_empty()
    }

    pub fn is_repository_scan_ready(&self) -> bool {
        self.repository_scan_ready
    }

    /// 读取 HEAD 或 index 中 `path` 的文本，建立/原位刷新修订文档并回填缓存。
    ///
    /// 缓存生命周期全部由 GitStore 管理：加载即回填，HEAD/index 变化时 commit_job 清空。
    /// 返回 `None` 表示该修订中文件不存在（同样写入缓存，避免调用方反复重试）。
    pub fn load_revision_document(
        &self,
        revision: GitRevision,
        path: &Path,
        cx: &App,
    ) -> Task<Option<Entity<LanguageBuffer>>> {
        let background = self.background.clone();
        let Ok(path) = canonicalize_path(path) else {
            return background.spawn(async { None });
        };
        let Some(repository) = self.repo_for_path(path.as_path()) else {
            return background.spawn(async { None });
        };
        let repository = repository.repository.clone();
        let generation = self
            .revision_generations
            .get(&revision)
            .copied()
            .unwrap_or_default();
        let Some(relative) =
            repository_working_directory(repository.as_ref()).relative_path(path.as_path())
        else {
            return background.spawn(async { None });
        };
        let revision_spec = match revision {
            GitRevision::Head => format!("HEAD:{relative}"),
            GitRevision::Index => format!(":{relative}"),
        };
        let loaded = background.spawn(async move {
            let contents = repository.load_revisions(&[&revision_spec]).ok()?;
            let content = contents.into_iter().next()??;
            Some(String::from_utf8_lossy(&content).into_owned())
        });
        let this = self.self_handle.clone();
        let language_registry = Arc::clone(&self.language_registry);
        cx.spawn(async move |cx| {
            let text = loaded.await;
            this.update(cx, |store, cx| {
                if store.revision_generations.get(&revision).copied() != Some(generation) {
                    return None;
                }
                store.store_revision_document(revision, path, text, &language_registry, cx)
            })
            .ok()
            .flatten()
        })
    }

    /// 建立或原位刷新修订文档；`text` 为 None 表示该修订中文件不存在。
    fn store_revision_document(
        &mut self,
        revision: GitRevision,
        path: AbsolutePathBuf,
        text: Option<String>,
        language_registry: &Arc<LanguageRegistry>,
        cx: &mut Context<Self>,
    ) -> Option<Entity<LanguageBuffer>> {
        let key = (revision, path.clone());
        let Some(text) = text else {
            // 缺失也要写入缓存：键存在表示“已加载”，避免调用方反复重试。
            self.revision_documents.insert(key, None);
            return None;
        };
        if let Some(Some(document)) = self.revision_documents.get(&key).cloned() {
            let snapshot = document.read(cx).text_snapshot();
            if snapshot_text(&snapshot) != text {
                document.update(cx, |document, cx| {
                    document
                        .replace_text(text, cx)
                        .expect("修订文档文本必须能原位刷新");
                });
            }
            return Some(document);
        }
        let buffer = Buffer::from_text(text, BufferConfig::default())
            .expect("修订文档文本必须能创建 Buffer");
        // 修订源的文件路径必须与工作区源一致（绝对），excerpt 定位、语言解析与导航按源路径匹配。
        let document = cx.new(|cx| {
            LanguageBuffer::new(
                buffer,
                Some(path.as_path().to_path_buf()),
                Arc::clone(language_registry),
                cx,
            )
        });
        self.revision_documents.insert(key, Some(document.clone()));
        Some(document)
    }

    /// 用给定文本原位刷新已加载的修订文档（乐观 index 写入与回滚）。
    fn update_revision_document_text(
        &mut self,
        revision: GitRevision,
        path: &AbsolutePathBuf,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (revision, path.clone());
        match self.revision_documents.get(&key).cloned() {
            Some(Some(document)) => {
                document.update(cx, |document, cx| {
                    document
                        .replace_text(text.to_string(), cx)
                        .expect("修订文档文本必须能原位刷新");
                });
            }
            // 缓存尚未建立时按乐观文本直接建立，语义与旧文本缓存一致。
            _ => {
                let buffer = Buffer::from_text(text.to_string(), BufferConfig::default())
                    .expect("修订文档文本必须能创建 Buffer");
                let document = cx.new(|cx| {
                    LanguageBuffer::new(
                        buffer,
                        Some(path.as_path().to_path_buf()),
                        Arc::clone(&self.language_registry),
                        cx,
                    )
                });
                self.revision_documents.insert(key, Some(document));
            }
        }
        let generation = self.revision_generations.entry(revision).or_insert(0);
        *generation = generation.wrapping_add(1).max(1);
    }

    /// 后台加载活动仓库的提交图数据（一次性读，不进 job 队列、不维护快照状态）。
    ///
    /// `offset` 为已跳过的提交数量：`None` 从历史开头开始，`Some(offset)` 从该位置继续；
    /// `limit` 为单批提交数上限。lane 布局由版本控制视图计算。
    /// 无活动仓库时返回空列表。仿 `load_revision_document` 的 `background.spawn` 一次性后台读模式。
    pub fn load_commit_graph(
        &self,
        offset: Option<usize>,
        limit: usize,
    ) -> Task<anyhow::Result<Vec<GraphCommit>>> {
        let background = self.background.clone();
        let Some(repository) = self.active_repository() else {
            return background.spawn(async { Ok(Vec::new()) });
        };
        let repository = repository.repository.clone();
        background.spawn(async move { repository.commit_graph(offset, limit) })
    }

    /// 读取缓存的修订文档；`None` 表示文件在该修订中缺失或尚未加载。
    ///
    /// 与 [`GitStore::revision_document_loaded`] 搭配区分“未加载”和“确实不存在”。
    pub fn revision_document(
        &self,
        revision: GitRevision,
        path: &Path,
    ) -> Option<Entity<LanguageBuffer>> {
        self.revision_documents
            .get(&(revision, canonicalize_path(path).ok()?))
            .and_then(|document| document.clone())
    }

    /// 该修订文档是否已经完成一次加载（缺失也算已加载）。
    pub fn revision_document_loaded(&self, revision: GitRevision, path: &Path) -> bool {
        let Ok(path) = canonicalize_path(path) else {
            return false;
        };
        self.revision_documents.contains_key(&(revision, path))
    }

    /// 读取缓存修订文档的全文；派生值，用于乐观写入的基准校验。
    fn revision_document_text(
        &self,
        revision: GitRevision,
        path: &Path,
        cx: &App,
    ) -> Option<Arc<str>> {
        let document = self.revision_document(revision, path)?;
        Some(Arc::from(
            snapshot_text(&document.read(cx).text_snapshot()).as_str(),
        ))
    }

    fn invalidate_revision_documents(&mut self, revision: GitRevision) {
        self.revision_documents
            .retain(|(cached_revision, path), _| {
                *cached_revision != revision
                    || (revision == GitRevision::Index
                        && self.optimistic_index_bases.contains_key(path))
            });
        let generation = self.revision_generations.entry(revision).or_insert(0);
        *generation = generation.wrapping_add(1).max(1);
        // head/index 文档变了：基于旧文档的共享 diff 全部失效。
        self.invalidate_shared_diffs(None);
    }

    fn invalidate_revision_documents_for_paths(
        &mut self,
        revision: GitRevision,
        paths: &[AbsolutePathBuf],
    ) {
        let changed_paths = paths.to_vec();
        self.revision_documents
            .retain(|(cached_revision, path), _| {
                *cached_revision != revision
                    || (revision == GitRevision::Index
                        && self.optimistic_index_bases.contains_key(path))
                    || !changed_paths
                        .iter()
                        .any(|changed_path| path.starts_with(changed_path))
            });
        let generation = self.revision_generations.entry(revision).or_insert(0);
        *generation = generation.wrapping_add(1).max(1);
        self.invalidate_shared_diffs(Some(paths));
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
                let paths: Vec<AbsolutePathBuf> =
                    std::mem::take(&mut self.paths_needing_status_update)
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
            GitJob::GitOperation { .. }
            | GitJob::CheckoutBranch { .. }
            | GitJob::CreateBranch { .. }
            | GitJob::DeleteBranch { .. } => {
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
                                            path.as_path().starts_with(rel.as_path())
                                                && matches(entry)
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
            GitJob::ResolveConflicts { paths } => {
                let (repositories, grouped_paths) = self.group_paths_by_repo(paths);
                Some(JobPreparation {
                    root,
                    repositories,
                    grouped_paths,
                    grouped_diff_requests: Vec::new(),
                })
            }
            GitJob::ApplyHunkEdits { path, .. } => {
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
        paths: &[AbsolutePathBuf],
    ) -> (Vec<Arc<dyn GitRepository>>, Vec<Vec<RelativePathBuf>>) {
        let mut repositories = Vec::with_capacity(self.repositories.len());
        let mut grouped_paths = vec![Vec::new(); self.repositories.len()];
        for (index, repository) in self.repositories.iter().enumerate() {
            repositories.push(repository.repository.clone());
            let workdir = repository_working_directory(repository.repository.as_ref());
            grouped_paths[index].extend(paths.iter().filter_map(|path| {
                // fs 事件路径可能未 canonicalize（如 macOS 的 /var → /private/var）。
                path.starts_with(&workdir)
                    .then(|| workdir.relative_path(path.as_path()))
                    .flatten()
            }));
        }
        (repositories, grouped_paths)
    }
}

/// GitStore 提供的变更块操作实现：把界面线程确定的编辑交给后台执行。
struct GitDiffOperations {
    store: WeakEntity<GitStore>,
    base: GitRevision,
}

impl DiffOperations for GitDiffOperations {
    fn supports_staging(&self) -> bool {
        self.base == GitRevision::Index
    }

    fn supports_unstaging(&self) -> bool {
        self.base == GitRevision::Head
    }

    fn supports_restore(&self) -> bool {
        self.base == GitRevision::Index
    }

    fn stage(&self, diff: Entity<BufferDiff>, ranges: Vec<Range<Anchor>>, cx: &mut App) {
        let Some(store) = self.store.upgrade() else {
            return;
        };
        store.update(cx, |store, cx| {
            if let Err(error) = store.apply_hunk_edits(GitHunkOperation::Stage, diff, ranges, cx) {
                cx.emit(GitStoreEvent::HunkOperationFailed(error));
            }
        });
    }

    fn unstage(&self, diff: Entity<BufferDiff>, ranges: Vec<Range<Anchor>>, cx: &mut App) {
        let Some(store) = self.store.upgrade() else {
            return;
        };
        store.update(cx, |store, cx| {
            if let Err(error) = store.apply_hunk_edits(GitHunkOperation::Unstage, diff, ranges, cx)
            {
                cx.emit(GitStoreEvent::HunkOperationFailed(error));
            }
        });
    }

    fn restore(&self, diff: Entity<BufferDiff>, ranges: Vec<Range<Anchor>>, cx: &mut App) {
        let Some(store) = self.store.upgrade() else {
            return;
        };
        store.update(cx, |store, cx| {
            if let Err(error) = store.apply_hunk_edits(GitHunkOperation::Restore, diff, ranges, cx)
            {
                cx.emit(GitStoreEvent::HunkOperationFailed(error));
            }
        });
    }
}

/// 路径归一化：把调用方可能未 canonicalize 的路径转换为可比较的绝对路径。
///
/// 路径及其祖先都已不存在等无法归一化的情况由调用方决定如何降级，这里不再静默保留原路径。
pub(super) fn canonicalize_path(path: &Path) -> std::io::Result<AbsolutePathBuf> {
    normalize_for_comparison(path)
}

/// 读取快照全文；修订文档派生文本的唯一转换点。
fn snapshot_text(snapshot: &Snapshot) -> String {
    snapshot
        .slice_text(
            TextRange::new(ByteOffset::ZERO, snapshot.len_bytes()).expect("全文范围必须有序"),
        )
        .expect("全文范围必须有效")
        .as_str()
        .to_owned()
}

impl EventEmitter<GitStoreEvent> for GitStore {}

struct JobPreparation {
    root: AbsolutePathBuf,
    repositories: Vec<Arc<dyn GitRepository>>,
    grouped_paths: Vec<Vec<RelativePathBuf>>,
    grouped_diff_requests: Vec<Vec<()>>,
}

#[cfg(test)]
#[path = "test/mod_tests.rs"]
mod tests;
