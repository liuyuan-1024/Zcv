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
use zcv_git::{DiffStat, FileStatus, GitRepository};

use super::worktree::{discover_git_repository, find_git_repositories};

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
}

/// 单个文件在某个仓库中的状态快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StatusEntry {
    pub(crate) status: FileStatus,
    /// 暂存 + 未暂存之和（面板展示改动计数用）。
    pub(crate) diff_stat: DiffStat,
    pub(crate) staged_diff_stat: DiffStat,
    pub(crate) unstaged_diff_stat: DiffStat,
}

/// 单个仓库的状态快照。
#[derive(Debug)]
pub(crate) struct RepositorySnapshot {
    pub(crate) branch: Option<String>,
    pub(crate) head: Option<String>,
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
    GitOperation(GitOperationKind),
}

/// 用户触发的 git 操作（由 top_bar 按钮发起，后台执行）。
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
    GitOperation(GitOperationKind),
}

impl GitJob {
    fn key(&self) -> GitJobKey {
        match self {
            GitJob::ReloadGitState => GitJobKey::ReloadGitState,
            GitJob::RefreshStatuses => GitJobKey::RefreshStatuses,
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
    statuses: zcv_git::GitStatus,
    staged: HashMap<PathBuf, DiffStat>,
    unstaged: HashMap<PathBuf, DiffStat>,
}

enum JobResult {
    Reload(Vec<ReloadScan>),
    Refresh(Vec<(usize, RefreshData)>),
    GitOperation(anyhow::Result<()>),
}

pub(crate) struct GitStore {
    root: PathBuf,
    repositories: Vec<Repository>,
    background: BackgroundExecutor,
    job_sender: async_channel::Sender<GitJob>,
    pending_jobs: HashMap<GitJobKey, ()>,
    paths_needing_status_update: BTreeSet<PathBuf>,
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
            background,
            job_sender,
            pending_jobs: HashMap::new(),
            paths_needing_status_update: BTreeSet::new(),
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

    /// 当前活动仓库的分支名（无仓库或空仓库时为 None）。
    pub(crate) fn current_branch(&self) -> Option<&str> {
        self.repositories
            .iter()
            .find(|repository| repository.snapshot.branch.is_some())
            .and_then(|repository| repository.snapshot.branch.as_deref())
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
            GitJob::GitOperation(_) => {
                // 对当前活动仓库执行（与 current_branch 同一选择逻辑）。
                let repository = self
                    .repositories
                    .iter()
                    .find(|repository| repository.snapshot.branch.is_some())
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
            // 仓库发现（同步文件系统遍历，放后台）：项目根是仓库 → 只用它，
            // 否则在根下找嵌套仓库。
            let discovered = discover_git_repository(&root)
                .ok()
                .flatten()
                .map(|repository| vec![repository])
                .or_else(|| find_git_repositories(&root).ok())
                .unwrap_or_default();
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
            };
            (path, entry)
        })
        .collect();
    RepositorySnapshot {
        branch,
        head,
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
    RefreshData {
        paths: paths.to_vec(),
        branch,
        head,
        statuses,
        staged,
        unstaged,
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
        let entry = StatusEntry {
            status,
            diff_stat: add_diff_stats(staged_diff_stat, unstaged_diff_stat),
            staged_diff_stat,
            unstaged_diff_stat,
        };
        statuses_changed |= statuses_by_path.insert(path, entry) != Some(entry);
    }

    let head_changed = prev.head != data.head || prev.branch != data.branch;
    (
        RepositorySnapshot {
            branch: data.branch,
            head: data.head,
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
            statuses_by_path: BTreeMap::from([
                (
                    PathBuf::from("a.txt"),
                    StatusEntry {
                        status: FileStatus::Untracked,
                        diff_stat: DiffStat::default(),
                        staged_diff_stat: DiffStat::default(),
                        unstaged_diff_stat: DiffStat::default(),
                    },
                ),
                (
                    PathBuf::from("sub/b.txt"),
                    StatusEntry {
                        status: FileStatus::Untracked,
                        diff_stat: DiffStat::default(),
                        staged_diff_stat: DiffStat::default(),
                        unstaged_diff_stat: DiffStat::default(),
                    },
                ),
            ]),
        };

        let data = RefreshData {
            paths: vec![PathBuf::from("a.txt"), PathBuf::from("sub")],
            branch: Some("master".into()),
            head: Some("old".into()),
            // a.txt 变干净（无输出 → 移除）；sub/c.txt 新增。
            statuses: zcv_git::GitStatus {
                statuses: vec![(PathBuf::from("sub/c.txt"), FileStatus::Untracked)],
            },
            staged: HashMap::new(),
            unstaged: HashMap::new(),
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
            statuses_by_path: BTreeMap::new(),
        };
        let data = RefreshData {
            paths: vec![PathBuf::from("a.txt")],
            branch: Some("master".into()),
            head: Some("new".into()),
            statuses: zcv_git::GitStatus::default(),
            staged: HashMap::new(),
            unstaged: HashMap::new(),
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
            store.status_for_path(&root.join("tracked.txt")).copied()
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
            let entry = store.status_for_path(&root.join("tracked.txt")).copied();
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
                store.status_for_path(&root.join("tracked.txt")).copied()
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
}
