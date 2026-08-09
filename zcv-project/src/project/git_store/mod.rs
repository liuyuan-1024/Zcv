//! git 状态编排：仓库发现、status 扫描与增量刷新、事件分发。
//!
//! Git 层的命令全部同步阻塞，这里负责把它们调度到后台线程，并维护每个仓库的状态快照（对齐 Zed `RepositorySnapshot`）。
//! 后台执行与扫描/合并纯函数在 [`background`] 子模块（可脱离 gpui 单测）。
//!
//! 刷新策略（对齐 Zed）：
//! - 全量（`ReloadGitState`）：仓库发现 + 每个仓库 head/status/双 diff_stat 全扫；
//! - 增量（`RefreshStatuses`）：只对变更路径重查，合并进旧快照，顺带重读 head/branch（外部 checkout 只触发 fs 事件走增量路径，不重读会滞后）。
//!
//! 同 key 的排队 job 直接丢弃（对齐 Zed `spawn_local_git_worker` 的 keyed job）。

mod background;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AsyncApp, BackgroundExecutor, Context, EventEmitter, Task, WeakEntity};
use zcv_buffer_diff::DiffHunk;
use zcv_git::{Branch, DiffStat, FileStatus, GitRepository};

use background::{JobResult, execute_job, merge_refresh, repo_relative_path};

/// 一次增量刷新最多累积的路径数，超过则升级为全量扫描。
const MAX_INCREMENTAL_PATHS: usize = 500;

/// 仓库状态变化的通知。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitStoreEvent {
    /// 仓库集合发生变化（发现/消失）。
    Repositories,
    /// 文件状态或 diff 统计发生变化。
    Statuses,
    /// 当前分支、HEAD 或分支列表发生变化。
    Head,
    /// 活动仓库变化（跟随焦点文件切换；订阅方重读 `current_branch()`，无需 payload）。
    ActiveRepositoryChanged,
}

/// 单个文件在某个仓库中的状态快照。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub status: FileStatus,
    /// 暂存 + 未暂存之和（面板展示改动计数用）。
    pub diff_stat: DiffStat,
    pub staged_diff_stat: DiffStat,
    pub unstaged_diff_stat: DiffStat,
    /// 行级 diff hunks（None = 尚未查询；Some([]) = 已查询且无变化）。
    /// 全量扫描不查，按需查询（打开文件 / 增量刷新时）。
    pub hunks: Option<Arc<[DiffHunk]>>,
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
}

/// 单个仓库的状态快照。
#[derive(Debug)]
pub struct RepositorySnapshot {
    pub branch: Option<String>,
    pub head: Option<String>,
    /// 最近一次提交的 subject（首行；无提交时为 None）。
    /// 底部提交区显示用，status 扫描时顺手读取（对齐 Zed branch scan 的 `%(contents:subject)`）。
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

pub struct Repository {
    pub repository: Arc<dyn GitRepository>,
    snapshot: RepositorySnapshot,
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

/// 用户触发的 git 操作（fetch/pull/push，由 UI 发起，后台执行）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GitOperationKind {
    Fetch,
    Pull,
    Push,
}

#[derive(Clone, Debug)]
pub(super) enum GitJob {
    ReloadGitState,
    RefreshStatuses,
    RefreshHunks,
    GitOperation(GitOperationKind),
    GitInit,
    StageFiles { stage: bool, paths: Vec<PathBuf> },
    Commit { message: String },
    Uncommit,
    CheckoutBranch { name: String },
    CreateBranch { name: String },
}

impl GitJob {
    fn key(&self) -> GitJobKey {
        match self {
            GitJob::ReloadGitState => GitJobKey::ReloadGitState,
            GitJob::RefreshStatuses => GitJobKey::RefreshStatuses,
            GitJob::RefreshHunks => GitJobKey::RefreshHunks,
            GitJob::GitOperation(operation) => GitJobKey::GitOperation(*operation),
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
}

pub struct GitStore {
    root: PathBuf,
    repositories: Vec<Repository>,
    /// 活动仓库（按 working_directory 标识）：分支显示与 fetch/pull/push 等 git 操作的目标。
    /// 用 working_directory 而非索引：全量扫描重建 Vec，索引不稳定。
    active_workdir: Option<PathBuf>,
    /// HEAD 文本缓存（删除块展开的被删除行来源；HEAD 变化时清空）。
    committed_text_cache: HashMap<PathBuf, Arc<str>>,
    /// uncommit 成功后暂存的被撤销消息（Head 事件后由面板读取填回提交信息编辑器）。
    pending_uncommitted_message: Option<String>,
    background: BackgroundExecutor,
    job_sender: async_channel::Sender<GitJob>,
    pending_jobs: HashMap<GitJobKey, ()>,
    paths_needing_status_update: BTreeSet<PathBuf>,
    paths_needing_hunks: BTreeSet<PathBuf>,
    _job_task: Task<()>,
}

impl GitStore {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        // 仓库的 working_directory 来自 canonicalize，root 同样归一化，保证路径前缀匹配一致。
        let root = canonicalize_path(&root);
        let background = cx.background_executor().clone();
        let (job_sender, job_receiver) = async_channel::unbounded::<GitJob>();
        // 单 worker 循环（照 fs_task 先例）：顺序处理 job，每个 job 在后台线程执行 git 命令，结果提交回 UI 线程。
        let job_task = cx.spawn(|this: WeakEntity<Self>, asynccx: &mut AsyncApp| {
            let mut cx = asynccx.clone();
            async move {
                while let Ok(job) = job_receiver.recv().await {
                    let Some(this) = this.upgrade() else {
                        break;
                    };
                    let Some(prepared) = this
                        .update(&mut cx, |store, _| store.prepare_job(&job))
                        .ok()
                        .flatten()
                    else {
                        continue;
                    };
                    let result = cx
                        .background_executor()
                        .spawn(execute_job(
                            prepared.root,
                            job.clone(),
                            prepared.repositories,
                            prepared.grouped_paths,
                        ))
                        .await;
                    let _ = this.update(&mut cx, |store, cx| store.commit_job(&job, result, cx));
                }
            }
        });

        Self {
            root,
            repositories: Vec::new(),
            active_workdir: None,
            committed_text_cache: HashMap::new(),
            pending_uncommitted_message: None,
            background,
            job_sender,
            pending_jobs: HashMap::new(),
            paths_needing_status_update: BTreeSet::new(),
            paths_needing_hunks: BTreeSet::new(),
            _job_task: job_task,
        }
    }

    /// 全量扫描：重新发现仓库并重扫所有状态（初始扫描与结构性变化时调用）。
    pub fn schedule_scan(&mut self, _cx: &mut Context<Self>) {
        self.paths_needing_status_update.clear();
        self.schedule_job(GitJob::ReloadGitState);
    }

    /// 后台执行用户触发的 git 操作（fetch/pull/push），完成后重新扫描。
    ///
    /// 仓库尚未扫描完成（首次打开项目）时只触发扫描，操作由用户稍后重试。
    pub fn run_operation(&mut self, operation: GitOperationKind, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            log::warn!(
                "git 仓库尚未就绪，跳过 {:?}（等待首次扫描完成后重试）",
                operation
            );
            self.schedule_scan(cx);
            return;
        }
        self.schedule_job(GitJob::GitOperation(operation));
    }

    /// 在项目根初始化 git 仓库（空态面板按钮触发），完成后重新扫描以发现新仓库。
    pub fn git_init(&mut self, _cx: &mut Context<Self>) {
        self.schedule_job(GitJob::GitInit);
    }

    /// 暂存路径（面板复选框勾选触发；`git update-index`），完成后自动重新扫描。
    pub fn stage_paths(&mut self, paths: Vec<PathBuf>, _cx: &mut Context<Self>) {
        self.schedule_job(GitJob::StageFiles { stage: true, paths });
    }

    /// 取消暂存路径（面板复选框取消勾选触发；`git reset`），完成后自动重新扫描。
    pub fn unstage_paths(&mut self, paths: Vec<PathBuf>, _cx: &mut Context<Self>) {
        self.schedule_job(GitJob::StageFiles {
            stage: false,
            paths,
        });
    }

    /// 提交暂存内容（消息来自面板提交信息编辑器）；无已暂存改动时自动暂存全部已跟踪改动。
    ///
    /// 成功后重扫，Head/Statuses 事件驱动面板清空编辑器并刷新上次提交信息。
    pub fn commit(&mut self, message: String, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            log::warn!("git 仓库尚未就绪，跳过 commit（等待首次扫描完成后重试）");
            self.schedule_scan(cx);
            return;
        }
        self.schedule_job(GitJob::Commit { message });
    }

    /// 撤销最近一次提交（`git reset --soft HEAD^`），被撤销消息填回提交信息编辑器。
    pub fn uncommit(&mut self, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            log::warn!("git 仓库尚未就绪，跳过 uncommit（等待首次扫描完成后重试）");
            self.schedule_scan(cx);
            return;
        }
        self.schedule_job(GitJob::Uncommit);
    }

    /// 切换活动仓库到指定本地分支（分支选择器确认触发），完成后自动重扫。
    pub fn checkout_branch(&mut self, name: String, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            log::warn!("git 仓库尚未就绪，跳过 checkout（等待首次扫描完成后重试）");
            self.schedule_scan(cx);
            return;
        }
        self.schedule_job(GitJob::CheckoutBranch { name });
    }

    /// 以当前 HEAD 为基创建并切换分支（选择器"创建分支"行触发），完成后自动重扫。
    pub fn create_branch(&mut self, name: String, cx: &mut Context<Self>) {
        if self.repositories.is_empty() {
            log::warn!("git 仓库尚未就绪，跳过 create_branch（等待首次扫描完成后重试）");
            self.schedule_scan(cx);
            return;
        }
        self.schedule_job(GitJob::CreateBranch { name });
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

    /// 增量刷新：对变更路径重查状态（fs 事件、保存操作后调用）。
    pub fn refresh_statuses_for_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        // 调用方传入的路径可能未 canonicalize，与归一化后的 root 比较前先归一化。
        let paths: Vec<PathBuf> = paths
            .iter()
            .map(|path| canonicalize_path(path))
            .filter(|path| path.starts_with(&self.root))
            .collect();
        if paths.is_empty() {
            return;
        }
        self.paths_needing_status_update.extend(paths);
        // 路径累积超过阈值时升级为全量扫描，避免单次增量 job 过大。
        if self.paths_needing_status_update.len() >= MAX_INCREMENTAL_PATHS {
            self.schedule_scan(cx);
        } else {
            self.schedule_job(GitJob::RefreshStatuses);
        }
    }

    /// 查询文件状态（最长前缀匹配仓库；不在任何仓库中时为 None）。
    pub fn status_for_path(&self, path: &Path) -> Option<&StatusEntry> {
        let path = canonicalize_path(path);
        let repository = self.repo_for_path(&path)?;
        let relative = repo_relative_path(repository.repository.working_directory(), &path)?;
        repository.snapshot.statuses_by_path.get(&relative)
    }

    /// 文件的行级 diff hunks（None = 尚未查询；Some([]) = 已查询且无变化）。
    pub fn hunks_for_path(&self, path: &Path) -> Option<Arc<[DiffHunk]>> {
        self.status_for_path(path)?.hunks.clone()
    }

    /// 按需请求指定路径的 hunks（打开文件、或 Statuses 事件后补齐时调用）。
    ///
    /// prepare 阶段过滤：只对「已跟踪且尚未查询」的路径发起 diff，untracked/忽略/已查询路径直接丢弃（untracked 永不查询 → 永不画 marker，对齐 Zed）。
    pub fn request_hunks(&mut self, paths: &[PathBuf], _cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = paths
            .iter()
            .map(|path| canonicalize_path(path))
            .filter(|path| path.starts_with(&self.root))
            .collect();
        if paths.is_empty() {
            return;
        }
        self.paths_needing_hunks.extend(paths);
        self.schedule_job(GitJob::RefreshHunks);
    }

    /// 查询目录的聚合状态（对齐 Zed `git_traversal` 的目录摘要）。
    ///
    /// 目录自身被忽略时直接返回；
    /// 否则取子项中优先级最高的状态（conflict > deleted > modified > added/untracked）。
    /// 被忽略的子项不参与聚合——目录不应因内部忽略文件而淡显。
    pub fn status_for_directory(&self, path: &Path) -> Option<FileStatus> {
        let path = canonicalize_path(path);
        let repository = self.repo_for_path(&path)?;
        let relative = repo_relative_path(repository.repository.working_directory(), &path)?;
        let statuses = &repository.snapshot.statuses_by_path;
        // 目录自身条目（--ignored=matching 下仅忽略目录会有目录级条目）。
        if let Some(entry) = statuses.get(&relative)
            && entry.status.is_ignored()
        {
            return Some(FileStatus::Ignored);
        }
        // 聚合子项：BTreeMap 有序，以目录为前缀的键连续排列在 range(..) 中。
        let mut best: Option<FileStatus> = None;
        for (key, entry) in statuses.range(relative.clone()..) {
            if !key.starts_with(&relative) {
                break;
            }
            if key == &relative || entry.status.is_ignored() {
                continue;
            }
            if best.is_none_or(|current| entry.status.priority() > current.priority()) {
                best = Some(entry.status);
            }
        }
        best
    }

    /// 当前活动仓库的分支名（无仓库、active 未建立或活动仓库为空仓库时为 None）。
    pub fn current_branch(&self) -> Option<&str> {
        self.active_workdir
            .as_ref()
            .and_then(|workdir| self.repo_by_workdir(workdir))
            .and_then(|repository| repository.snapshot.branch.as_deref())
    }

    /// 活动仓库的本地分支列表（无仓库、active 未建立时为 None；空仓库为空列表）。
    ///
    /// 与 current_branch 同仓库选择策略，保证分支 glyph 与列表一致。
    pub fn active_branch_list(&self) -> Option<&[Branch]> {
        self.active_workdir
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

    /// 取出 uncommit 成功后被撤销的提交消息（面板在 Head 事件后调用填回编辑器）。
    pub fn take_pending_uncommitted_message(&mut self) -> Option<String> {
        self.pending_uncommitted_message.take()
    }

    /// 按 working_directory 查找仓库。
    fn repo_by_workdir(&self, workdir: &Path) -> Option<&Repository> {
        self.repositories
            .iter()
            .find(|repository| repository.repository.working_directory() == workdir)
    }

    /// 操作目标仓库：active 已建立时用它，否则回退「首个有分支的仓库」→「首个」。
    ///
    /// fetch/pull/push、提交、uncommit 与底部提交信息显示共用此选择（对齐 Zed：操作以 active 仓库为目标，空仓库也执行）。
    fn active_repository(&self) -> Option<&Repository> {
        self.active_workdir
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
        self.active_workdir
            .as_ref()
            .and_then(|workdir| self.repo_by_workdir(workdir))
            .map(|repository| RemoteOperationState {
                has_remote: repository.snapshot.has_remote,
                ahead: repository.snapshot.ahead,
                behind: repository.snapshot.behind,
            })
            .unwrap_or_default()
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
        if self.active_workdir.as_deref() != Some(workdir.as_path()) {
            self.active_workdir = Some(workdir);
            cx.emit(GitStoreEvent::ActiveRepositoryChanged);
        }
    }

    /// 是否已发现至少一个 git 仓库（决定 git 相关 UI 是否可见）。
    pub fn has_repositories(&self) -> bool {
        !self.repositories.is_empty()
    }

    /// 读取 HEAD 中 `path` 的文本（diff base），不在仓库/无 HEAD 时为 None。
    pub fn load_committed_text(&self, path: &Path) -> Task<Option<String>> {
        let background = self.background.clone();
        let path = canonicalize_path(path);
        let Some(repository) = self.repo_for_path(&path) else {
            return background.spawn(async { None });
        };
        let repository = repository.repository.clone();
        let Some(relative) = repo_relative_path(repository.working_directory(), &path) else {
            return background.spawn(async { None });
        };
        let revision = format!("HEAD:{}", relative.to_string_lossy());
        background.spawn(async move {
            let contents = repository.load_revisions(&[&revision]).ok()?;
            let content = contents.into_iter().next()??;
            Some(String::from_utf8_lossy(&content).into_owned())
        })
    }

    /// 读取缓存的 HEAD 文本（删除块展开用；未预取时为 None）。
    pub fn committed_text(&self, path: &Path) -> Option<Arc<str>> {
        self.committed_text_cache
            .get(&canonicalize_path(path))
            .cloned()
    }

    /// 缓存 HEAD 文本（HEAD 变化时由 commit_job 清空）。
    pub fn cache_committed_text(&mut self, path: &Path, text: Arc<str>) {
        self.committed_text_cache
            .insert(canonicalize_path(path), text);
    }

    fn schedule_job(&mut self, job: GitJob) {
        // 同 key 的 job 已在队列/执行中时，丢弃新 job（路径已累积在paths_needing_status_update，由正在执行的 job 统一消费）。
        let key = job.key();
        if self.pending_jobs.contains_key(&key) {
            return;
        }
        self.pending_jobs.insert(key, ());
        let _ = self.job_sender.try_send(job);
    }

    /// UI 线程：取出 job 需要的共享数据（后台线程不能访问 Entity 状态）。
    fn prepare_job(&mut self, job: &GitJob) -> Option<JobPreparation> {
        let root = self.root.clone();
        match job {
            GitJob::ReloadGitState => Some(JobPreparation {
                root,
                repositories: Vec::new(),
                grouped_paths: Vec::new(),
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
                })
            }
            GitJob::RefreshHunks => {
                // 按需 hunk 查询：只对「已跟踪且尚未查询」的路径发起 diff，untracked/忽略/已查询路径直接丢弃（untracked 永不查询 → 永不画 marker）。
                let paths = std::mem::take(&mut self.paths_needing_hunks);
                let mut repositories = Vec::with_capacity(self.repositories.len());
                let mut grouped_paths = vec![Vec::new(); self.repositories.len()];
                for (index, repository) in self.repositories.iter().enumerate() {
                    repositories.push(repository.repository.clone());
                    let workdir = repository.repository.working_directory();
                    grouped_paths[index].extend(paths.iter().filter_map(|path| {
                        let path = canonicalize_path(path);
                        let relative = path
                            .starts_with(workdir)
                            .then(|| repo_relative_path(workdir, &path))
                            .flatten()?;
                        let entry = repository.snapshot.statuses_by_path.get(&relative)?;
                        (matches!(entry.status, FileStatus::Tracked { .. })
                            && entry.hunks.is_none())
                        .then_some(relative)
                    }));
                }
                Some(JobPreparation {
                    root,
                    repositories,
                    grouped_paths,
                })
            }
            GitJob::GitOperation(_)
            | GitJob::CheckoutBranch { .. }
            | GitJob::CreateBranch { .. } => {
                // 作用于活动仓库（对齐 Zed：fetch/pull/push 以 active 仓库为目标，空仓库也执行；
                // 分支操作与 top_bar 显示的分支同仓库）。
                let repository = self.active_repository()?.repository.clone();
                Some(JobPreparation {
                    root,
                    repositories: vec![repository],
                    grouped_paths: Vec::new(),
                })
            }
            // init 作用于项目根，不依赖既有仓库集合。
            GitJob::GitInit => Some(JobPreparation {
                root,
                repositories: Vec::new(),
                grouped_paths: Vec::new(),
            }),
            GitJob::StageFiles { stage, paths } => {
                let (repositories, grouped_paths) = self.group_paths_by_repo(paths);
                // 目录路径展开为该仓库快照内状态匹配的文件（git update-index 不递归目录，直接传目录会失败；
                // 对齐 Zed：目录勾选收集其下文件路径逐个暂存）。
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
                })
            }
            // 提交/撤销提交：作用于活动仓库（与 GitOperation 同选择策略）；
            // commit 时无已暂存改动则自动暂存全部已跟踪改动（对齐 Zed，未跟踪文件须手动暂存）。
            GitJob::Commit { .. } | GitJob::Uncommit => {
                let repository = self.active_repository()?;
                let has_staged = repository
                    .snapshot
                    .statuses_by_path
                    .values()
                    .any(|entry| entry.status.has_staged());
                let paths_to_stage = if matches!(job, GitJob::Uncommit) || has_staged {
                    Vec::new()
                } else {
                    repository
                        .snapshot
                        .statuses_by_path
                        .iter()
                        .filter(|(_, entry)| {
                            !entry.status.is_created() && entry.status.has_unstaged()
                        })
                        .map(|(path, _)| path.clone())
                        .collect()
                };
                Some(JobPreparation {
                    root,
                    repositories: vec![repository.repository.clone()],
                    grouped_paths: vec![paths_to_stage],
                })
            }
        }
    }

    /// UI 线程：提交 job 结果，比对旧快照后发出对应事件。
    fn commit_job(&mut self, job: &GitJob, result: JobResult, cx: &mut Context<Self>) {
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
                    .active_workdir
                    .as_ref()
                    .filter(|workdir| new_work_dirs.contains(*workdir))
                    .cloned()
                    .or_else(|| {
                        self.repositories.first().map(|repository| {
                            repository.repository.working_directory().to_path_buf()
                        })
                    });
                if self.active_workdir != new_active {
                    self.active_workdir = new_active;
                    cx.emit(GitStoreEvent::ActiveRepositoryChanged);
                }
                log::info!("git 状态已刷新：{} 个仓库", self.repositories.len());
            }
            (GitJob::RefreshStatuses, JobResult::Refresh(refreshed)) => {
                let mut statuses_changed = false;
                let mut head_changed = false;
                // hunks 解耦：状态变化后统一排 RefreshHunks（job 队列同 key 去重，高频编辑合并），不再在状态刷新里内联计算。
                let mut hunks_paths = Vec::new();
                for (index, data) in refreshed {
                    let Some(repository) = self.repositories.get_mut(index) else {
                        continue;
                    };
                    hunks_paths.extend(data.paths.iter().cloned());
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
                    self.paths_needing_hunks.extend(hunks_paths);
                    self.schedule_job(GitJob::RefreshHunks);
                    cx.emit(GitStoreEvent::Statuses);
                }
            }
            (GitJob::RefreshHunks, JobResult::RefreshHunks(refreshed)) => {
                let mut statuses_changed = false;
                for (index, hunks_by_path) in refreshed {
                    let Some(repository) = self.repositories.get_mut(index) else {
                        continue;
                    };
                    for (path, hunks) in hunks_by_path {
                        // get_mut 守卫：prepare → commit 间隙条目可能已消失（如外部删除），不复活。
                        if let Some(entry) = repository.snapshot.statuses_by_path.get_mut(&path) {
                            entry.hunks = Some(Arc::from(hunks));
                            statuses_changed = true;
                        }
                    }
                }
                if statuses_changed {
                    cx.emit(GitStoreEvent::Statuses);
                }
            }
            (
                job @ (GitJob::GitOperation(_)
                | GitJob::GitInit
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
        self.pending_jobs.remove(&job.key());
    }

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
fn canonicalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

impl EventEmitter<GitStoreEvent> for GitStore {}

struct JobPreparation {
    root: PathBuf,
    repositories: Vec<Arc<dyn GitRepository>>,
    grouped_paths: Vec<Vec<PathBuf>>,
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{run_git, test_git_repo};
    use super::*;
    use std::fs;

    use gpui::AppContext;
    use zcv_git::StatusCode;

    #[gpui::test]
    fn scan_discovers_repository_and_reports_status(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        fs::write(root.join("tracked.txt"), "已修改\n").expect("应修改文件");

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let entry = cx.read_entity(&git_store, |store, _| {
            store.status_for_path(&root.join("tracked.txt")).cloned()
        });
        let entry = entry.expect("应有 tracked.txt 的状态");
        assert!(entry.status.is_modified());
        assert!(entry.diff_stat.added >= 1);

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
            cx.update(|cx| cx.new(|cx| GitStore::new(temp_dir.path().to_path_buf(), cx)));
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
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
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

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
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
    fn load_committed_text_returns_head_content(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 修改工作区文件，HEAD 内容应仍是初始版本。
        fs::write(root.join("tracked.txt"), "已修改\n").expect("应修改文件");
        let text = cx.executor().block(cx.read_entity(&git_store, |store, _| {
            store.load_committed_text(&root.join("tracked.txt"))
        }));
        assert_eq!(text.as_deref(), Some("第一行\n第二行\n"));
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

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
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

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
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

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
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

        let git_store = cx.new(|cx| GitStore::new(root.clone(), cx));
        git_store.update(cx, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked(); // 首次扫描完成，repositories 就绪。
        let ready = cx.read_entity(&git_store, |store, _| !store.repositories.is_empty());
        assert!(ready, "首次扫描后 repositories 应就绪");

        // 本地新提交 → run_operation(Push) → 后台 job 推送。
        fs::write(root.join("new.txt"), "新文件\n").expect("应写入文件");
        run_git(&root, &["add", "new.txt"]);
        run_git(&root, &["commit", "-q", "-m", "新提交"]);
        git_store.update(cx, |store, cx| {
            store.run_operation(GitOperationKind::Push, cx);
        });
        cx.run_until_parked();
        let job_done = cx.read_entity(&git_store, |store, _| {
            !store
                .pending_jobs
                .contains_key(&GitJobKey::GitOperation(GitOperationKind::Push))
        });
        assert!(job_done, "push job 应已完成");

        // 远程应指向本地 HEAD。
        let rev = |dir: &Path| {
            String::from_utf8_lossy(
                &std::process::Command::new("git")
                    .args(["rev-parse", "master"])
                    .current_dir(dir)
                    .output()
                    .expect("应能读取 HEAD")
                    .stdout,
            )
            .trim()
            .to_string()
        };
        assert_eq!(rev(&remote), rev(&root), "push 后远程应指向本地 HEAD");
    }

    #[gpui::test]
    fn git_init_then_scan_discovers_repository(cx: &mut gpui::TestAppContext) {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let root = temp_dir.path().to_path_buf();

        let git_store = cx.new(|cx| GitStore::new(root.clone(), cx));
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

        let git_store = cx.new(|cx| GitStore::new(root.clone(), cx));
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
    }

    #[gpui::test]
    fn stage_paths_expands_directory_to_matching_files(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        // src 下的已跟踪修改 + 未跟踪新文件 + 子目录文件。
        std::fs::create_dir_all(root.join("src/sub")).expect("应创建目录");
        fs::write(root.join("src/a.txt"), "改动的 a\n").expect("应写入文件");
        fs::write(root.join("src/new.txt"), "新文件\n").expect("应写入文件");
        fs::write(root.join("src/sub/b.txt"), "改动的 b\n").expect("应写入文件");

        let git_store = cx.new(|cx| GitStore::new(root.clone(), cx));
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
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let tracked = root.join("tracked.txt");
        // 全量扫描不查 hunks：初始为 None。
        assert!(
            cx.read_entity(&git_store, |store, _| store.hunks_for_path(&tracked))
                .is_none()
        );

        // 外部修改第 2 行 → 按需请求 hunks。
        fs::write(&tracked, "第一行\n改了第二行\n").expect("应修改文件");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(std::slice::from_ref(&tracked), cx)
        });
        cx.run_until_parked();
        cx.update_entity(&git_store, |store, cx| {
            store.request_hunks(std::slice::from_ref(&tracked), cx)
        });
        cx.run_until_parked();

        let hunks = cx
            .read_entity(&git_store, |store, _| store.hunks_for_path(&tracked))
            .expect("请求后应有 hunks");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].range, 1..2);
        assert_eq!(hunks[0].kind, zcv_buffer_diff::DiffHunkKind::Modified);
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

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
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

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
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
            store.run_operation(GitOperationKind::Push, cx);
        });
        cx.run_until_parked();
        let job_done = cx.read_entity(&git_store, |store, _| {
            !store
                .pending_jobs
                .contains_key(&GitJobKey::GitOperation(GitOperationKind::Push))
        });
        assert!(job_done, "push job 应已完成");

        // 远端应指向嵌套仓库 HEAD（而非根仓库）。
        let rev = |dir: &Path| {
            String::from_utf8_lossy(
                &std::process::Command::new("git")
                    .args(["rev-parse", "master"])
                    .current_dir(dir)
                    .output()
                    .expect("应能读取 HEAD")
                    .stdout,
            )
            .trim()
            .to_string()
        };
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

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
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

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 与远程同步：有 remote，无 ahead/behind。
        let state = cx.read_entity(&git_store, |store, _| store.remote_operation_state());
        assert_eq!(
            state,
            RemoteOperationState {
                has_remote: true,
                ahead: 0,
                behind: 0
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
            store.run_operation(GitOperationKind::Push, cx);
        });
        cx.run_until_parked();
        cx.run_until_parked(); // 等 push 完成后触发的重新扫描落地。
        let state = cx.read_entity(&git_store, |store, _| store.remote_operation_state());
        assert_eq!(
            state,
            RemoteOperationState {
                has_remote: true,
                ahead: 0,
                behind: 0
            },
            "push 后 ahead 应归零"
        );
    }

    #[gpui::test]
    fn remote_operation_state_defaults_without_remote(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let state = cx.read_entity(&git_store, |store, _| store.remote_operation_state());
        assert_eq!(state, RemoteOperationState::default());
    }

    #[gpui::test]
    fn request_hunks_skips_untracked_files(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 未跟踪文件：请求后仍为 None（永不查询，对齐 Zed 不画 untracked marker）。
        let untracked = root.join("untracked.txt");
        fs::write(&untracked, "新的\n").expect("应写入文件");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(std::slice::from_ref(&untracked), cx)
        });
        cx.run_until_parked();
        cx.update_entity(&git_store, |store, cx| {
            store.request_hunks(std::slice::from_ref(&untracked), cx)
        });
        cx.run_until_parked();

        assert!(
            cx.read_entity(&git_store, |store, _| store.hunks_for_path(&untracked))
                .is_none()
        );
    }

    #[gpui::test]
    fn scan_reports_branch_list(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        run_git(&root, &["checkout", "-q", "-b", "feature"]);

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
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

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
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
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
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

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
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
            cx.update(|cx| cx.new(|cx| GitStore::new(temp_dir.path().to_path_buf(), cx)));
        cx.update_entity(&git_store, |store, cx| {
            store.checkout_branch("master".into(), cx);
            store.create_branch("feature".into(), cx);
        });
        cx.run_until_parked();
    }
}
