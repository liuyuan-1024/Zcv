//! git 状态编排：仓库发现、status 扫描与增量刷新、事件分发。
//!
//! zcv-git 层的命令全部同步阻塞，这里负责把它们调度到后台线程，并维护每个仓库的状态快照（对齐 Zed `RepositorySnapshot`）。
//!
//! 刷新策略（对齐 Zed）：
//! - 全量（`ReloadGitState`）：仓库发现 + 每个仓库 head/status/双 diff_stat 全扫；
//! - 增量（`RefreshStatuses`）：只对变更路径重查，合并进旧快照，顺带重读 head/branch（外部 checkout 只触发 fs 事件走增量路径，不重读会滞后）。
//!
//! 同 key 的排队 job 直接丢弃（对齐 Zed `spawn_local_git_worker` 的 keyed job）。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use gpui::{AsyncApp, BackgroundExecutor, Context, EventEmitter, Task, WeakEntity};
use zcv_git::{DiffHunk, DiffStat, FileStatus, GitRepository};

use super::worktree::discover_repositories;

/// 一次增量刷新最多累积的路径数，超过则升级为全量扫描。
const MAX_INCREMENTAL_PATHS: usize = 500;

/// 仓库状态变化的通知。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitStoreEvent {
    /// 仓库集合发生变化（发现/消失）。
    Repositories,
    /// 文件状态或 diff 统计发生变化。
    Statuses,
    /// 当前分支或 HEAD 发生变化。
    Head,
    /// 活动仓库变化（跟随焦点文件切换；订阅方重读 `current_branch()`，无需 payload）。
    ActiveRepositoryChanged,
}

/// 单个文件在某个仓库中的状态快照。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StatusEntry {
    pub(crate) status: FileStatus,
    /// 暂存 + 未暂存之和（面板展示改动计数用）。
    pub(crate) diff_stat: DiffStat,
    pub(crate) staged_diff_stat: DiffStat,
    pub(crate) unstaged_diff_stat: DiffStat,
    /// 行级 diff hunks（None = 尚未查询；Some([]) = 已查询且无变化）。
    /// 全量扫描不查，按需查询（打开文件 / 增量刷新时）。
    pub(crate) hunks: Option<Arc<[DiffHunk]>>,
}

/// 活动仓库的远程操作状态（remote 配置与 upstream 领先/落后计数）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteOperationState {
    /// 是否配置了 remote（无 remote 时 fetch/pull/push 均不可用）。
    pub(crate) has_remote: bool,
    /// 本地领先 upstream 的提交数（可推送数）。
    pub(crate) ahead: usize,
    /// 本地落后 upstream 的提交数（可拉取数）。
    pub(crate) behind: usize,
}

/// 单个仓库的状态快照。
#[derive(Debug)]
pub(crate) struct RepositorySnapshot {
    pub(crate) branch: Option<String>,
    pub(crate) head: Option<String>,
    /// 是否配置了 remote。
    pub(crate) has_remote: bool,
    /// 当前分支相对 upstream 的领先/落后计数（无 upstream 时为 0）。
    pub(crate) ahead: usize,
    pub(crate) behind: usize,
    /// 相对仓库根的路径 → 状态。
    pub(crate) statuses_by_path: BTreeMap<PathBuf, StatusEntry>,
}

pub(crate) struct Repository {
    pub(crate) repository: Arc<dyn GitRepository>,
    snapshot: RepositorySnapshot,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum GitJobKey {
    ReloadGitState,
    RefreshStatuses,
    RefreshHunks,
    GitOperation(GitOperationKind),
}

/// 用户触发的 git 操作（fetch/pull/push，由 UI 发起，后台执行）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum GitOperationKind {
    Fetch,
    Pull,
    Push,
}

#[derive(Clone, Copy)]
enum GitJob {
    ReloadGitState,
    RefreshStatuses,
    RefreshHunks,
    GitOperation(GitOperationKind),
}

impl GitJob {
    fn key(&self) -> GitJobKey {
        match self {
            GitJob::ReloadGitState => GitJobKey::ReloadGitState,
            GitJob::RefreshStatuses => GitJobKey::RefreshStatuses,
            GitJob::RefreshHunks => GitJobKey::RefreshHunks,
            GitJob::GitOperation(operation) => GitJobKey::GitOperation(*operation),
        }
    }
}

/// 全量扫描的产出（一个仓库）。
struct ReloadScan {
    working_directory: PathBuf,
    repository: Arc<dyn GitRepository>,
    snapshot: RepositorySnapshot,
}

/// 增量刷新的原始数据（后台查询结果）。
struct RefreshData {
    paths: Vec<PathBuf>,
    branch: Option<String>,
    head: Option<String>,
    has_remote: bool,
    ahead: usize,
    behind: usize,
    statuses: zcv_git::GitStatus,
    staged: HashMap<PathBuf, DiffStat>,
    unstaged: HashMap<PathBuf, DiffStat>,
    /// 已跟踪路径的行级 diff hunks（只查增量路径中状态为 Tracked 的文件）。
    hunks: Vec<(PathBuf, Vec<DiffHunk>)>,
}

/// 按需 hunk 查询结果：仓库索引 → 路径 → hunks。
type HunksByRepo = Vec<(usize, Vec<(PathBuf, Vec<DiffHunk>)>)>;

enum JobResult {
    Reload(Vec<ReloadScan>),
    Refresh(Vec<(usize, RefreshData)>),
    RefreshHunks(HunksByRepo),
    GitOperation(anyhow::Result<()>),
}

pub(crate) struct GitStore {
    root: PathBuf,
    repositories: Vec<Repository>,
    /// 活动仓库（按 working_directory 标识）：分支显示与 fetch/pull/push 等 git 操作的目标。
    /// 用 working_directory 而非索引：全量扫描重建 Vec，索引不稳定。
    active_workdir: Option<PathBuf>,
    background: BackgroundExecutor,
    job_sender: async_channel::Sender<GitJob>,
    pending_jobs: HashMap<GitJobKey, ()>,
    paths_needing_status_update: BTreeSet<PathBuf>,
    paths_needing_hunks: BTreeSet<PathBuf>,
    _job_task: Task<()>,
}

impl GitStore {
    pub(crate) fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
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
                            job,
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
            background,
            job_sender,
            pending_jobs: HashMap::new(),
            paths_needing_status_update: BTreeSet::new(),
            paths_needing_hunks: BTreeSet::new(),
            _job_task: job_task,
        }
    }

    /// 全量扫描：重新发现仓库并重扫所有状态（初始扫描与结构性变化时调用）。
    pub(crate) fn schedule_scan(&mut self, _cx: &mut Context<Self>) {
        self.paths_needing_status_update.clear();
        self.schedule_job(GitJob::ReloadGitState);
    }

    /// 后台执行用户触发的 git 操作（fetch/pull/push），完成后重新扫描。
    ///
    /// 仓库尚未扫描完成（首次打开项目）时只触发扫描，操作由用户稍后重试。
    pub(crate) fn run_operation(&mut self, operation: GitOperationKind, cx: &mut Context<Self>) {
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

    /// 增量刷新：对变更路径重查状态（fs 事件、保存操作后调用）。
    pub(crate) fn refresh_statuses_for_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
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
    pub(crate) fn status_for_path(&self, path: &Path) -> Option<&StatusEntry> {
        let path = canonicalize_path(path);
        let repository = self.repo_for_path(&path)?;
        let relative = repo_relative_path(repository.repository.working_directory(), &path)?;
        repository.snapshot.statuses_by_path.get(&relative)
    }

    /// 文件的行级 diff hunks（None = 尚未查询；Some([]) = 已查询且无变化）。
    pub(crate) fn hunks_for_path(&self, path: &Path) -> Option<Arc<[DiffHunk]>> {
        self.status_for_path(path)?.hunks.clone()
    }

    /// 按需请求指定路径的 hunks（打开文件、或 Statuses 事件后补齐时调用）。
    ///
    /// prepare 阶段过滤：只对「已跟踪且尚未查询」的路径发起 diff，untracked/忽略/
    /// 已查询路径直接丢弃（untracked 永不查询 → 永不画 marker，对齐 Zed）。
    pub(crate) fn request_hunks(&mut self, paths: &[PathBuf], _cx: &mut Context<Self>) {
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
    pub(crate) fn status_for_directory(&self, path: &Path) -> Option<FileStatus> {
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
    pub(crate) fn current_branch(&self) -> Option<&str> {
        self.active_workdir
            .as_ref()
            .and_then(|workdir| self.repo_by_workdir(workdir))
            .and_then(|repository| repository.snapshot.branch.as_deref())
    }

    /// 按 working_directory 查找仓库。
    fn repo_by_workdir(&self, workdir: &Path) -> Option<&Repository> {
        self.repositories
            .iter()
            .find(|repository| repository.repository.working_directory() == workdir)
    }

    /// 活动仓库的远程操作状态（可推送/可拉取判定依据）。
    pub(crate) fn remote_operation_state(&self) -> RemoteOperationState {
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
    pub(crate) fn set_active_repository_for_path(&mut self, path: &Path, cx: &mut Context<Self>) {
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
    pub(crate) fn has_repositories(&self) -> bool {
        !self.repositories.is_empty()
    }

    /// 读取 HEAD 中 `path` 的文本（diff base），不在仓库/无 HEAD 时为 None。
    pub(crate) fn load_committed_text(&self, path: &Path) -> Task<Option<String>> {
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

    fn schedule_job(&mut self, job: GitJob) {
        // 同 key 的 job 已在队列/执行中时，丢弃新 job（路径已累积在
        // paths_needing_status_update，由正在执行的 job 统一消费）。
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
                let paths = std::mem::take(&mut self.paths_needing_status_update);
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
                Some(JobPreparation {
                    root,
                    repositories,
                    grouped_paths,
                })
            }
            GitJob::RefreshHunks => {
                // 按需 hunk 查询：只对「已跟踪且尚未查询」的路径发起 diff，
                // untracked/忽略/已查询路径直接丢弃（untracked 永不查询 → 永不画 marker）。
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
            GitJob::GitOperation(_) => {
                // 作用于活动仓库（对齐 Zed：fetch/pull/push 以 active 仓库为目标，空仓库也执行）；
                // active 尚未建立（首次扫描前）时回退原有选择逻辑。
                let repository = self
                    .active_workdir
                    .as_ref()
                    .and_then(|workdir| self.repo_by_workdir(workdir))
                    .or_else(|| {
                        self.repositories
                            .iter()
                            .find(|repository| repository.snapshot.branch.is_some())
                    })
                    .or_else(|| self.repositories.first())?
                    .repository
                    .clone();
                Some(JobPreparation {
                    root,
                    repositories: vec![repository],
                    grouped_paths: Vec::new(),
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
                for (index, data) in refreshed {
                    let Some(repository) = self.repositories.get_mut(index) else {
                        continue;
                    };
                    let (snapshot, statuses, head) = merge_refresh(&repository.snapshot, data);
                    statuses_changed |= statuses;
                    head_changed |= head;
                    repository.snapshot = snapshot;
                }
                if head_changed {
                    cx.emit(GitStoreEvent::Head);
                }
                if statuses_changed {
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
            (GitJob::GitOperation(operation), JobResult::GitOperation(result)) => {
                match result {
                    Ok(()) => {
                        // 操作改变了引用/工作树：重新全量扫描，比对后发出 Head/Statuses 事件。
                        log::info!("git {:?} 成功", operation);
                        self.schedule_scan(cx);
                    }
                    Err(error) => {
                        log::warn!("git {:?} 失败：{error:#}", operation);
                    }
                }
            }
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

/// 后台线程：执行一个 job（所有 git 命令在这里同步阻塞运行）。
async fn execute_job(
    root: PathBuf,
    job: GitJob,
    repositories: Vec<Arc<dyn GitRepository>>,
    grouped_paths: Vec<Vec<PathBuf>>,
) -> JobResult {
    match job {
        GitJob::ReloadGitState => {
            // 仓库发现（同步文件系统遍历，放后台）：总是递归发现嵌套仓库，再合并 root 所在的外层仓库。
            let discovered = discover_repositories(&root).unwrap_or_default();
            let scans = discovered
                .into_iter()
                .map(|repository| {
                    let working_directory = repository.working_directory().to_path_buf();
                    let repository: Arc<dyn GitRepository> = Arc::new(repository);
                    let snapshot = scan_repository_sync(&repository);
                    ReloadScan {
                        working_directory,
                        repository,
                        snapshot,
                    }
                })
                .collect();
            JobResult::Reload(scans)
        }
        GitJob::RefreshStatuses => {
            let mut refreshed = Vec::new();
            for (index, repository) in repositories.into_iter().enumerate() {
                let paths = &grouped_paths[index];
                if paths.is_empty() {
                    continue;
                }
                refreshed.push((index, refresh_repository_data_sync(&repository, paths)));
            }
            JobResult::Refresh(refreshed)
        }
        GitJob::RefreshHunks => {
            let mut refreshed = Vec::new();
            for (index, repository) in repositories.into_iter().enumerate() {
                let paths = &grouped_paths[index];
                if paths.is_empty() {
                    continue;
                }
                refreshed.push((index, fetch_hunks_sync(&repository, paths)));
            }
            JobResult::RefreshHunks(refreshed)
        }
        GitJob::GitOperation(operation) => {
            let result = repositories
                .first()
                .map(|repository| match operation {
                    GitOperationKind::Fetch => repository.fetch(),
                    GitOperationKind::Pull => repository.pull(),
                    GitOperationKind::Push => repository.push(),
                })
                .unwrap_or(Ok(()));
            JobResult::GitOperation(result)
        }
    }
}

/// 后台线程：逐路径查询行级 diff hunks（每路径一个 git 进程）。
///
/// 单路径失败仅跳过该路径（保留 None 等待下次事件重试，自愈），不中断整批。
fn fetch_hunks_sync(
    repository: &Arc<dyn GitRepository>,
    paths: &[PathBuf],
) -> Vec<(PathBuf, Vec<DiffHunk>)> {
    paths
        .iter()
        .filter_map(|path| match repository.diff_hunks(path) {
            Ok(hunks) => Some((path.clone(), hunks)),
            Err(error) => {
                log::warn!("读取 diff hunks 失败（{path:?}）：{error}");
                None
            }
        })
        .collect()
}

/// 后台线程：全量扫描一个仓库（head + status + 双 diff_stat）。
fn scan_repository_sync(repository: &Arc<dyn GitRepository>) -> RepositorySnapshot {
    let (branch, head) = match repository.head() {
        Ok(head) => head,
        Err(error) => {
            log::warn!("读取 git head 失败：{error}");
            (None, None)
        }
    };
    let statuses = match repository.status(&[]) {
        Ok(statuses) => statuses,
        Err(error) => {
            log::warn!("读取 git status 失败：{error}");
            return RepositorySnapshot {
                branch,
                head,
                has_remote: false,
                ahead: 0,
                behind: 0,
                statuses_by_path: BTreeMap::new(),
            };
        }
    };
    // 无 HEAD（空仓库）时 `--cached HEAD` 会报错，跳过暂存统计。
    let staged = if head.is_some() {
        repository.diff_stat(true, &[]).unwrap_or_default()
    } else {
        HashMap::new()
    };
    let unstaged = repository.diff_stat(false, &[]).unwrap_or_default();
    let statuses_by_path = statuses
        .statuses
        .into_iter()
        .map(|(path, status)| {
            let staged_diff_stat = staged.get(&path).copied().unwrap_or_default();
            let unstaged_diff_stat = unstaged.get(&path).copied().unwrap_or_default();
            let entry = StatusEntry {
                status,
                diff_stat: add_diff_stats(staged_diff_stat, unstaged_diff_stat),
                staged_diff_stat,
                unstaged_diff_stat,
                // 全量扫描不查 hunks，打开文件时按需查询。
                hunks: None,
            };
            (path, entry)
        })
        .collect();
    RepositorySnapshot {
        branch,
        head,
        has_remote: repository.has_remote().unwrap_or(false),
        ahead: statuses.branch.as_ref().map_or(0, |branch| branch.ahead),
        behind: statuses.branch.as_ref().map_or(0, |branch| branch.behind),
        statuses_by_path,
    }
}

/// 后台线程：对变更路径重查状态（head 一并重读，兜底外部 checkout）。
fn refresh_repository_data_sync(
    repository: &Arc<dyn GitRepository>,
    paths: &[PathBuf],
) -> RefreshData {
    let (branch, head) = match repository.head() {
        Ok(head) => head,
        Err(error) => {
            log::warn!("读取 git head 失败：{error}");
            (None, None)
        }
    };
    let statuses = repository.status(paths).unwrap_or_default();
    let staged = if head.is_some() {
        repository.diff_stat(true, paths).unwrap_or_default()
    } else {
        HashMap::new()
    };
    let unstaged = repository.diff_stat(false, paths).unwrap_or_default();
    // hunks 与 status 同批次：只对「已跟踪」路径查 diff（untracked/忽略/干净路径跳过）。
    let hunks = statuses
        .statuses
        .iter()
        .filter_map(|(path, status)| {
            if !matches!(status, FileStatus::Tracked { .. }) {
                return None;
            }
            match repository.diff_hunks(path) {
                Ok(hunks) => Some((path.clone(), hunks)),
                Err(error) => {
                    log::warn!("读取 diff hunks 失败（{path:?}）：{error}");
                    None
                }
            }
        })
        .collect();
    RefreshData {
        paths: paths.to_vec(),
        branch,
        head,
        // status 失败归零与 head 失败语义一致（瞬态，下次成功刷新自愈）。
        has_remote: repository.has_remote().unwrap_or(false),
        ahead: statuses.branch.as_ref().map_or(0, |branch| branch.ahead),
        behind: statuses.branch.as_ref().map_or(0, |branch| branch.behind),
        statuses,
        staged,
        unstaged,
        hunks,
    }
}

/// 纯函数：把增量刷新数据合并进旧快照。
///
/// 只更新刷新路径覆盖的条目：新 status 中的路径插入/更新，旧快照中
/// 不再变化的路径（本次 status 无输出）移除。返回（新快照，状态是否
/// 变化，head 是否变化）。
fn merge_refresh(prev: &RepositorySnapshot, data: RefreshData) -> (RepositorySnapshot, bool, bool) {
    let mut statuses_by_path = prev.statuses_by_path.clone();
    let mut statuses_changed = false;

    // 移除旧条目：BTreeMap 有序，以 path 为前缀的键连续排列在 range(path..) 中。
    for path in &data.paths {
        let mut to_remove = Vec::new();
        for (key, _) in statuses_by_path.range(path.clone()..) {
            if key.starts_with(path) {
                to_remove.push(key.clone());
            } else {
                break;
            }
        }
        for key in to_remove {
            statuses_changed |= statuses_by_path.remove(&key).is_some();
        }
    }

    for (path, status) in data.statuses.statuses {
        let staged_diff_stat = data.staged.get(&path).copied().unwrap_or_default();
        let unstaged_diff_stat = data.unstaged.get(&path).copied().unwrap_or_default();
        let hunks = data
            .hunks
            .iter()
            .find(|(hunk_path, _)| hunk_path == &path)
            .map(|(_, hunks)| Arc::from(hunks.as_slice()));
        let entry = StatusEntry {
            status,
            diff_stat: add_diff_stats(staged_diff_stat, unstaged_diff_stat),
            staged_diff_stat,
            unstaged_diff_stat,
            hunks,
        };
        let replaced = statuses_by_path.insert(path.clone(), entry.clone());
        statuses_changed |= replaced != Some(entry);
    }

    // ahead/behind/has_remote 纳入比对：fetch/push 后计数变化必须触发事件，否则订阅方无法感知。
    let head_changed = prev.head != data.head
        || prev.branch != data.branch
        || prev.has_remote != data.has_remote
        || prev.ahead != data.ahead
        || prev.behind != data.behind;
    (
        RepositorySnapshot {
            branch: data.branch,
            head: data.head,
            has_remote: data.has_remote,
            ahead: data.ahead,
            behind: data.behind,
            statuses_by_path,
        },
        statuses_changed,
        head_changed,
    )
}

fn add_diff_stats(a: DiffStat, b: DiffStat) -> DiffStat {
    DiffStat {
        added: a.added + b.added,
        deleted: a.deleted + b.deleted,
    }
}

/// 绝对路径 → 仓库相对路径（unix 分隔符，git 参数格式）。
fn repo_relative_path(working_directory: &Path, path: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(working_directory).ok()?;
    let mut unix = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => unix.push(name),
            _ => return None,
        }
    }
    Some(unix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use gpui::AppContext;

    use tempfile::TempDir;

    /// 创建带一个初始提交的临时 git 仓库，返回 (仓库根, 目录句柄)。
    fn test_repo() -> (PathBuf, TempDir) {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let root = temp_dir.path().to_path_buf();
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        fs::write(root.join("tracked.txt"), "第一行\n第二行\n").expect("应写入初始文件");
        run_in(&root, &["git", "add", "tracked.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        (root, temp_dir)
    }

    fn run_in(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .output()
            .expect("应执行成功");
        assert!(
            output.status.success(),
            "命令 {:?} 失败：{}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn merge_refresh_replaces_changed_paths_and_keeps_rest() {
        let prev = RepositorySnapshot {
            branch: Some("master".into()),
            head: Some("old".into()),
            has_remote: true,
            ahead: 1,
            behind: 0,
            statuses_by_path: BTreeMap::from([
                (
                    PathBuf::from("a.txt"),
                    StatusEntry {
                        status: FileStatus::Untracked,
                        diff_stat: DiffStat::default(),
                        staged_diff_stat: DiffStat::default(),
                        unstaged_diff_stat: DiffStat::default(),
                        hunks: None,
                    },
                ),
                (
                    PathBuf::from("sub/b.txt"),
                    StatusEntry {
                        status: FileStatus::Untracked,
                        diff_stat: DiffStat::default(),
                        staged_diff_stat: DiffStat::default(),
                        unstaged_diff_stat: DiffStat::default(),
                        hunks: None,
                    },
                ),
            ]),
        };

        let data = RefreshData {
            paths: vec![PathBuf::from("a.txt"), PathBuf::from("sub")],
            branch: Some("master".into()),
            head: Some("old".into()),
            has_remote: true,
            ahead: 1,
            behind: 0,
            // a.txt 变干净（无输出 → 移除）；sub/c.txt 新增。
            statuses: zcv_git::GitStatus {
                statuses: vec![(PathBuf::from("sub/c.txt"), FileStatus::Untracked)],
                branch: None,
            },
            staged: HashMap::new(),
            unstaged: HashMap::new(),
            hunks: Vec::new(),
        };

        let (snapshot, statuses_changed, head_changed) = merge_refresh(&prev, data);
        assert!(statuses_changed);
        assert!(!head_changed);
        assert!(!snapshot.statuses_by_path.contains_key(Path::new("a.txt")));
        assert!(
            !snapshot
                .statuses_by_path
                .contains_key(Path::new("sub/b.txt"))
        );
        assert!(
            snapshot
                .statuses_by_path
                .contains_key(Path::new("sub/c.txt"))
        );
    }

    #[test]
    fn merge_refresh_detects_head_changes() {
        let prev = RepositorySnapshot {
            branch: Some("master".into()),
            head: Some("old".into()),
            has_remote: false,
            ahead: 0,
            behind: 0,
            statuses_by_path: BTreeMap::new(),
        };
        let data = RefreshData {
            paths: vec![PathBuf::from("a.txt")],
            branch: Some("master".into()),
            head: Some("new".into()),
            has_remote: false,
            ahead: 0,
            behind: 0,
            statuses: zcv_git::GitStatus::default(),
            staged: HashMap::new(),
            unstaged: HashMap::new(),
            hunks: Vec::new(),
        };

        let (_, statuses_changed, head_changed) = merge_refresh(&prev, data);
        assert!(!statuses_changed);
        assert!(head_changed);
    }

    #[test]
    fn relative_path_converts_to_unix_style() {
        assert_eq!(
            repo_relative_path(Path::new("/repo"), Path::new("/repo/src/main.rs")),
            Some(PathBuf::from("src/main.rs"))
        );
        assert_eq!(
            repo_relative_path(Path::new("/repo"), Path::new("/other/file.rs")),
            None
        );
    }

    #[gpui::test]
    fn scan_discovers_repository_and_reports_status(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_repo();
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
        run_in(temp_dir.path(), &["git", "init", "-q", "-b", "master"]);

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
        let (root, _temp) = test_repo();
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
        let (root, _temp) = test_repo();
        // 第二个分支。
        run_in(&root, &["git", "checkout", "-q", "-b", "feature"]);
        fs::write(root.join("tracked.txt"), "feature 内容\n").expect("应写入");
        run_in(&root, &["git", "commit", "-q", "-am", "feature"]);

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 外部 checkout 回 master：只触发 fs 事件 → 增量刷新应重读 head。
        run_in(&root, &["git", "checkout", "-q", "master"]);
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(&[root.join("tracked.txt")], cx)
        });
        cx.run_until_parked();

        let branch = cx.read_entity(&git_store, |store, _| {
            store.current_branch().map(str::to_string)
        });
        assert_eq!(branch.as_deref(), Some("master"));
    }

    #[gpui::test]
    fn load_committed_text_returns_head_content(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_repo();
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
        let (root, _temp) = test_repo();
        fs::create_dir_all(root.join("src")).expect("应创建目录");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("应创建文件");
        run_in(&root, &["git", "add", "src/main.rs"]);
        run_in(&root, &["git", "commit", "-q", "-m", "add src"]);
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
        let (root, _temp) = test_repo();
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
        let (root, _temp) = test_repo();
        fs::write(root.join(".gitignore"), ".DS_Store\n").expect("应写入 .gitignore");
        fs::create_dir_all(root.join("assets")).expect("应创建目录");
        fs::write(root.join("assets/.DS_Store"), "x").expect("应创建忽略文件");
        fs::write(root.join("assets/logo.png"), "x").expect("应创建文件");
        run_in(&root, &["git", "add", "assets/logo.png"]);
        run_in(&root, &["git", "commit", "-q", "-m", "add assets"]);

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
        run_in(
            temp_dir.path(),
            &["git", "init", "-q", "--bare", remote.to_str().unwrap()],
        );
        let root = temp_dir.path().join("work");
        fs::create_dir(&root).expect("应创建工作仓库目录");
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        fs::write(root.join("tracked.txt"), "内容\n").expect("应写入初始文件");
        run_in(&root, &["git", "add", "tracked.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        run_in(
            &root,
            &["git", "remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&root, &["git", "push", "-q", "-u", "origin", "master"]);

        let git_store = cx.new(|cx| GitStore::new(root.clone(), cx));
        git_store.update(cx, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked(); // 首次扫描完成，repositories 就绪。
        let ready = cx.read_entity(&git_store, |store, _| !store.repositories.is_empty());
        assert!(ready, "首次扫描后 repositories 应就绪");

        // 本地新提交 → run_operation(Push) → 后台 job 推送。
        fs::write(root.join("new.txt"), "新文件\n").expect("应写入文件");
        run_in(&root, &["git", "add", "new.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "新提交"]);
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
    fn request_hunks_fills_hunks_on_demand(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_repo();
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
            store.refresh_statuses_for_paths(&[tracked.clone()], cx)
        });
        cx.run_until_parked();
        cx.update_entity(&git_store, |store, cx| {
            store.request_hunks(&[tracked.clone()], cx)
        });
        cx.run_until_parked();

        let hunks = cx
            .read_entity(&git_store, |store, _| store.hunks_for_path(&tracked))
            .expect("请求后应有 hunks");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].range, 1..2);
        assert_eq!(hunks[0].kind, zcv_git::DiffHunkKind::Modified);
    }

    #[gpui::test]
    fn active_repository_follows_focused_path(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_repo();
        let nested = root.join("nested");
        fs::create_dir(&nested).expect("应创建嵌套目录");
        run_in(&nested, &["git", "init", "-q", "-b", "feature"]);
        run_in(
            &nested,
            &["git", "config", "user.email", "test@example.com"],
        );
        run_in(&nested, &["git", "config", "user.name", "Test User"]);
        fs::write(nested.join("n.txt"), "嵌套\n").expect("应写入嵌套文件");
        run_in(&nested, &["git", "add", "n.txt"]);
        run_in(&nested, &["git", "commit", "-q", "-m", "nested initial"]);

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
        let (root, _temp) = test_repo();
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let remote = temp_dir.path().join("remote.git");
        run_in(
            temp_dir.path(),
            &["git", "init", "-q", "--bare", remote.to_str().unwrap()],
        );
        let nested = root.join("nested");
        fs::create_dir(&nested).expect("应创建嵌套目录");
        run_in(&nested, &["git", "init", "-q", "-b", "master"]);
        run_in(
            &nested,
            &["git", "config", "user.email", "test@example.com"],
        );
        run_in(&nested, &["git", "config", "user.name", "Test User"]);
        fs::write(nested.join("n.txt"), "嵌套\n").expect("应写入嵌套文件");
        run_in(&nested, &["git", "add", "n.txt"]);
        run_in(&nested, &["git", "commit", "-q", "-m", "nested initial"]);
        run_in(
            &nested,
            &["git", "remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&nested, &["git", "push", "-q", "-u", "origin", "master"]);

        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // active 切到嵌套仓库，本地新提交后 push。
        cx.update_entity(&git_store, |store, cx| {
            store.set_active_repository_for_path(&nested.join("n.txt"), cx);
        });
        fs::write(nested.join("new.txt"), "新提交\n").expect("应写入文件");
        run_in(&nested, &["git", "add", "new.txt"]);
        run_in(&nested, &["git", "commit", "-q", "-m", "新提交"]);
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
        let (outer, _temp) = test_repo();
        let root = outer.join("proj");
        fs::create_dir(&root).expect("应创建项目目录");
        let nested = root.join("nested");
        fs::create_dir(&nested).expect("应创建嵌套目录");
        run_in(&nested, &["git", "init", "-q", "-b", "feature"]);
        run_in(
            &nested,
            &["git", "config", "user.email", "test@example.com"],
        );
        run_in(&nested, &["git", "config", "user.name", "Test User"]);
        fs::write(nested.join("n.txt"), "嵌套\n").expect("应写入嵌套文件");
        run_in(&nested, &["git", "add", "n.txt"]);
        run_in(&nested, &["git", "commit", "-q", "-m", "nested initial"]);

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
        run_in(
            temp_dir.path(),
            &["git", "init", "-q", "--bare", remote.to_str().unwrap()],
        );
        let root = temp_dir.path().join("work");
        fs::create_dir(&root).expect("应创建工作仓库目录");
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        fs::write(root.join("tracked.txt"), "内容\n").expect("应写入初始文件");
        run_in(&root, &["git", "add", "tracked.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        run_in(
            &root,
            &["git", "remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&root, &["git", "push", "-q", "-u", "origin", "master"]);

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
        run_in(&root, &["git", "add", "new.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "本地提交"]);
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
        let (root, _temp) = test_repo();
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        let state = cx.read_entity(&git_store, |store, _| store.remote_operation_state());
        assert_eq!(state, RemoteOperationState::default());
    }

    #[gpui::test]
    fn request_hunks_skips_untracked_files(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_repo();
        let git_store = cx.update(|cx| cx.new(|cx| GitStore::new(root.clone(), cx)));
        cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
        cx.run_until_parked();

        // 未跟踪文件：请求后仍为 None（永不查询，对齐 Zed 不画 untracked marker）。
        let untracked = root.join("untracked.txt");
        fs::write(&untracked, "新的\n").expect("应写入文件");
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(&[untracked.clone()], cx)
        });
        cx.run_until_parked();
        cx.update_entity(&git_store, |store, cx| {
            store.request_hunks(&[untracked.clone()], cx)
        });
        cx.run_until_parked();

        assert!(
            cx.read_entity(&git_store, |store, _| store.hunks_for_path(&untracked))
                .is_none()
        );
    }
}
