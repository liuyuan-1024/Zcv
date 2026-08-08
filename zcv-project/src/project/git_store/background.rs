//! 后台执行层：所有 git 命令在这里同步阻塞运行，扫描/合并为纯函数（可脱离 gpui 单测）。

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use zcv_buffer_diff::DiffHunk;
use zcv_git::{Branch, DiffStat, GitRepository};

use super::{GitJob, GitOperationKind, RepositorySnapshot, StatusEntry};
use crate::project::worktree::discover_repositories;

/// 后台 job 的执行结果。
pub(super) enum JobResult {
    Reload(Vec<ReloadScan>),
    Refresh(Vec<(usize, RefreshData)>),
    RefreshHunks(HunksByRepo),
    GitOperation(anyhow::Result<()>),
    /// uncommit 的结果（被撤销提交的完整消息，供填回提交信息编辑器）。
    Uncommit(anyhow::Result<Option<String>>),
}

/// 全量扫描的产出（一个仓库）。
pub(super) struct ReloadScan {
    pub(super) working_directory: PathBuf,
    pub(super) repository: Arc<dyn GitRepository>,
    pub(super) snapshot: RepositorySnapshot,
}

/// 增量刷新的原始数据（后台查询结果）。
pub(super) struct RefreshData {
    pub(super) paths: Vec<PathBuf>,
    /// 本轮是否重查过 head/branch/remote（快路径：纯文件变化不重读）。
    pub(super) head_queried: bool,
    pub(super) branch: Option<String>,
    pub(super) head: Option<String>,
    pub(super) last_commit_message: Option<String>,
    pub(super) has_remote: bool,
    pub(super) ahead: usize,
    pub(super) behind: usize,
    /// 本地分支列表（head_queried 为 false 时为空 vec，merge 不得误判变化）。
    pub(super) branches: Vec<Branch>,
    pub(super) statuses: zcv_git::GitStatus,
    pub(super) staged: HashMap<PathBuf, DiffStat>,
    pub(super) unstaged: HashMap<PathBuf, DiffStat>,
    pub(super) hunks: Vec<(PathBuf, Vec<DiffHunk>)>,
}

/// 按需 hunk 查询结果：仓库索引 → 路径 → hunks。
pub(super) type HunksByRepo = Vec<(usize, Vec<(PathBuf, Vec<DiffHunk>)>)>;

/// 后台线程：执行一个 job（所有 git 命令在这里同步阻塞运行）。
pub(super) async fn execute_job(
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
        // 分支操作：切换分支 / 以当前 HEAD 为基创建并切换（作用于活动仓库）。
        GitJob::CheckoutBranch { name } => {
            let result = repositories
                .first()
                .map(|repository| repository.checkout(&name))
                .unwrap_or(Ok(()));
            JobResult::GitOperation(result)
        }
        GitJob::CreateBranch { name } => {
            let result = repositories
                .first()
                .map(|repository| repository.create_branch(&name, None))
                .unwrap_or(Ok(()));
            JobResult::GitOperation(result)
        }
        // 项目根无仓库时初始化；fallback 分支名对齐 Zed 的 "main"。
        GitJob::GitInit => JobResult::GitOperation(zcv_git::init(&root, "main")),
        // 暂存/取消暂存：按仓库分组执行；任一仓库失败即中断并上报。
        GitJob::StageFiles { stage, .. } => {
            let mut result = Ok(());
            for (index, repository) in repositories.into_iter().enumerate() {
                let paths = &grouped_paths[index];
                if paths.is_empty() {
                    continue;
                }
                let outcome = if stage {
                    repository.stage_paths(paths)
                } else {
                    repository.unstage_paths(paths)
                };
                if let Err(error) = outcome {
                    result = Err(error);
                    break;
                }
            }
            JobResult::GitOperation(result)
        }
        // 提交：prepare 已算好需要先暂存的路径（无已暂存改动时 = 全部已跟踪改动），随后 commit。
        GitJob::Commit { message } => {
            let result = repositories.first().map(|repository| {
                let paths = grouped_paths
                    .first()
                    .map_or(&[][..], |paths| paths.as_slice());
                if !paths.is_empty() {
                    repository.stage_paths(paths)?;
                }
                repository.commit(&message)
            });
            JobResult::GitOperation(result.unwrap_or(Ok(())))
        }
        // 撤销提交：被撤销消息随结果回传，UI 线程暂存供面板填回编辑器。
        GitJob::Uncommit => {
            let result = repositories
                .first()
                .map(|repository| repository.uncommit())
                .unwrap_or(Ok(None));
            JobResult::Uncommit(result)
        }
    }
}

/// 后台线程：批量查询路径的行级 diff hunks（单进程）。
///
/// 整批失败时返回空（保留 None 等待下次事件重试，自愈）。
fn fetch_hunks_sync(
    repository: &Arc<dyn GitRepository>,
    paths: &[PathBuf],
) -> Vec<(PathBuf, Vec<DiffHunk>)> {
    match repository.diff_hunks_for_paths(paths) {
        Ok(hunks) => hunks,
        Err(error) => {
            log::warn!("读取 diff hunks 失败：{error}");
            Vec::new()
        }
    }
}

/// 后台线程：全量扫描一个仓库（head_commit + status + 双 diff_stat + 分支列表）。
///
/// branch 名取自 status 头行（零附加进程）；head oid 与最近提交 subject 由 head_commit 一次查询；
/// 分支列表单独 for-each-ref（分支选择器数据源）。
fn scan_repository_sync(repository: &Arc<dyn GitRepository>) -> RepositorySnapshot {
    let (head, last_commit_message) = match repository.head_commit() {
        Ok(commit) => commit,
        Err(error) => {
            log::warn!("读取 git head 失败：{error}");
            (None, None)
        }
    };
    let statuses = match repository.status(&[]) {
        Ok(statuses) => statuses,
        Err(error) => {
            log::warn!("读取 git status 失败：{error}");
            // status 失败时分支名不可得（来自头行），置 None（瞬态，下次刷新自愈）。
            return RepositorySnapshot {
                branch: None,
                head,
                last_commit_message,
                has_remote: false,
                ahead: 0,
                behind: 0,
                branch_list: Vec::new(),
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
    let branch_list = match repository.branches() {
        Ok(branches) => branches,
        Err(error) => {
            log::warn!("读取 git 分支列表失败：{error}");
            Vec::new()
        }
    };
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
        branch: statuses
            .branch
            .as_ref()
            .and_then(|branch| branch.branch.clone()),
        head,
        last_commit_message,
        has_remote: repository.has_remote().unwrap_or(false),
        ahead: statuses.branch.as_ref().map_or(0, |branch| branch.ahead),
        behind: statuses.branch.as_ref().map_or(0, |branch| branch.behind),
        branch_list,
        statuses_by_path,
    }
}

/// 后台线程：对变更路径重查状态。
///
/// 快路径：批次不含 `.git` 相关路径时跳过 head/branch/remote 重读
/// （纯文件变化不涉及引用；`.git` 相关变化仍全查，兜底外部 checkout）。
fn refresh_repository_data_sync(
    repository: &Arc<dyn GitRepository>,
    paths: &[PathBuf],
) -> RefreshData {
    let touches_git = paths.iter().any(|path| is_git_state_path(path));
    let statuses = repository.status(paths).unwrap_or_default();
    // 分支名来自 status 头行（零附加进程）；head oid 与最近提交 subject 由 head_commit 一次查询。
    let (head, last_commit_message) = if touches_git {
        match repository.head_commit() {
            Ok(commit) => commit,
            Err(error) => {
                log::warn!("读取 git head 失败：{error}");
                (None, None)
            }
        }
    } else {
        (None, None)
    };
    let branch = touches_git
        .then(|| {
            statuses
                .branch
                .as_ref()
                .and_then(|branch| branch.branch.clone())
        })
        .flatten();
    // 分支列表与 head 同批次重读：checkout/新建分支都落在 .git 路径上（fs 事件已过滤），外部操作经增量路径即可刷新选择器列表（对齐"外部 checkout 不重读会滞后"的既有语义）。
    let branches = if touches_git {
        match repository.branches() {
            Ok(branches) => branches,
            Err(error) => {
                log::warn!("读取 git 分支列表失败：{error}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    // diff_stat 不依赖 head 变量：无 HEAD 仓库 `--cached HEAD` 报错时按空处理（与 scan 语义一致）。
    let staged = repository.diff_stat(true, paths).unwrap_or_default();
    let unstaged = repository.diff_stat(false, paths).unwrap_or_default();
    RefreshData {
        paths: paths.to_vec(),
        head_queried: touches_git,
        branch,
        head,
        last_commit_message,
        // status 失败归零与 head 失败语义一致（瞬态，下次成功刷新自愈）。
        has_remote: touches_git && repository.has_remote().unwrap_or(false),
        ahead: statuses.branch.as_ref().map_or(0, |branch| branch.ahead),
        behind: statuses.branch.as_ref().map_or(0, |branch| branch.behind),
        branches,
        statuses,
        staged,
        unstaged,
        hunks: Vec::new(),
    }
}

/// 仓库相对路径是否为 `.git` 状态相关（快路径判定：`.git` 下一切路径都算）。
fn is_git_state_path(rel: &Path) -> bool {
    rel.components()
        .next()
        .is_some_and(|component| component.as_os_str() == ".git")
}

/// 纯函数：把增量刷新数据合并进旧快照（原位更新，不克隆整张状态表）。
///
/// 只更新刷新路径覆盖的条目：新 status 中的路径插入/更新，旧快照中
/// 不再变化的路径（本次 status 无输出）移除。
/// `head_queried` 为 false 时跳过 head 相关比对（快路径，保留旧值）。
/// 返回（状态是否变化，head 是否变化）。
pub(super) fn merge_refresh(prev: &mut RepositorySnapshot, data: RefreshData) -> (bool, bool) {
    let mut statuses_changed = false;

    // 移除旧条目：BTreeMap 有序，以 path 为前缀的键连续排列在 range(path..) 中。
    for path in &data.paths {
        let mut to_remove = Vec::new();
        for (key, _) in prev.statuses_by_path.range(path.clone()..) {
            if key.starts_with(path) {
                to_remove.push(key.clone());
            } else {
                break;
            }
        }
        for key in to_remove {
            statuses_changed |= prev.statuses_by_path.remove(&key).is_some();
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
        let replaced = prev.statuses_by_path.insert(path.clone(), entry.clone());
        statuses_changed |= replaced != Some(entry);
    }

    // ahead/behind/has_remote 与提交信息纳入比对：fetch/push/外部提交后变化必须触发事件，否则订阅方无法感知。
    // 分支列表只在 head_queried 时比对：快路径 data.branches 恒为空 vec，直接比对会误判"清空"。
    let head_changed = if data.head_queried {
        let changed = prev.head != data.head
            || prev.branch != data.branch
            || prev.last_commit_message != data.last_commit_message
            || prev.has_remote != data.has_remote
            || prev.ahead != data.ahead
            || prev.behind != data.behind
            || prev.branch_list != data.branches;
        if changed {
            prev.branch = data.branch;
            prev.head = data.head;
            prev.last_commit_message = data.last_commit_message;
            prev.has_remote = data.has_remote;
            prev.ahead = data.ahead;
            prev.behind = data.behind;
            prev.branch_list = data.branches;
        }
        changed
    } else {
        false
    };
    (statuses_changed, head_changed)
}

fn add_diff_stats(a: DiffStat, b: DiffStat) -> DiffStat {
    DiffStat {
        added: a.added + b.added,
        deleted: a.deleted + b.deleted,
    }
}

/// 绝对路径 → 仓库相对路径（unix 分隔符，git 参数格式）。
pub(super) fn repo_relative_path(working_directory: &Path, path: &Path) -> Option<PathBuf> {
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
    use std::collections::{BTreeMap, HashMap};
    use std::path::{Path, PathBuf};

    use zcv_git::{DiffStat, FileStatus};

    use super::*;
    use crate::project::git_store::{RepositorySnapshot, StatusEntry};

    #[test]
    fn merge_refresh_replaces_changed_paths_and_keeps_rest() {
        let prev = RepositorySnapshot {
            branch: Some("master".into()),
            head: Some("old".into()),
            last_commit_message: None,
            has_remote: true,
            ahead: 1,
            behind: 0,
            branch_list: Vec::new(),
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
            head_queried: true,
            branch: Some("master".into()),
            head: Some("old".into()),
            last_commit_message: None,
            has_remote: true,
            ahead: 1,
            behind: 0,
            branches: Vec::new(),
            // a.txt 变干净（无输出 → 移除）；sub/c.txt 新增。
            statuses: zcv_git::GitStatus {
                statuses: vec![(PathBuf::from("sub/c.txt"), FileStatus::Untracked)],
                branch: None,
            },
            staged: HashMap::new(),
            unstaged: HashMap::new(),
            hunks: Vec::new(),
        };

        let mut prev = prev;
        let (statuses_changed, head_changed) = merge_refresh(&mut prev, data);
        assert!(statuses_changed);
        assert!(!head_changed);
        assert!(!prev.statuses_by_path.contains_key(Path::new("a.txt")));
        assert!(!prev.statuses_by_path.contains_key(Path::new("sub/b.txt")));
        assert!(prev.statuses_by_path.contains_key(Path::new("sub/c.txt")));
    }

    #[test]
    fn merge_refresh_detects_head_changes() {
        let prev = RepositorySnapshot {
            branch: Some("master".into()),
            head: Some("old".into()),
            last_commit_message: None,
            has_remote: false,
            ahead: 0,
            behind: 0,
            branch_list: Vec::new(),
            statuses_by_path: BTreeMap::new(),
        };
        let data = RefreshData {
            paths: vec![PathBuf::from("a.txt")],
            head_queried: true,
            branch: Some("master".into()),
            head: Some("new".into()),
            last_commit_message: None,
            has_remote: false,
            ahead: 0,
            behind: 0,
            branches: Vec::new(),
            statuses: zcv_git::GitStatus::default(),
            staged: HashMap::new(),
            unstaged: HashMap::new(),
            hunks: Vec::new(),
        };

        let mut prev = prev;
        let (statuses_changed, head_changed) = merge_refresh(&mut prev, data);
        assert!(!statuses_changed);
        assert!(head_changed);
        assert_eq!(prev.head.as_deref(), Some("new"));
    }

    #[test]
    fn merge_refresh_without_head_query_keeps_head() {
        let mut prev = RepositorySnapshot {
            branch: Some("master".into()),
            head: Some("old".into()),
            last_commit_message: Some("旧提交".into()),
            has_remote: true,
            ahead: 2,
            behind: 1,
            branch_list: vec![Branch {
                name: "master".into(),
                is_head: true,
            }],
            statuses_by_path: BTreeMap::new(),
        };
        let data = RefreshData {
            paths: vec![PathBuf::from("a.txt")],
            // 快路径：未重查 head，合并时必须保留旧值且不触发 Head 事件。
            head_queried: false,
            branch: None,
            head: None,
            last_commit_message: None,
            has_remote: false,
            ahead: 0,
            behind: 0,
            // 快路径 branches 恒为空：不得覆盖既有列表，也不得误判"清空"变化。
            branches: Vec::new(),
            statuses: zcv_git::GitStatus::default(),
            staged: HashMap::new(),
            unstaged: HashMap::new(),
            hunks: Vec::new(),
        };

        let (statuses_changed, head_changed) = merge_refresh(&mut prev, data);
        assert!(!statuses_changed);
        assert!(!head_changed);
        assert_eq!(prev.head.as_deref(), Some("old"));
        assert_eq!(prev.branch.as_deref(), Some("master"));
        assert_eq!(prev.last_commit_message.as_deref(), Some("旧提交"));
        assert!(prev.has_remote);
        assert_eq!(prev.ahead, 2);
        assert_eq!(prev.behind, 1);
        assert_eq!(prev.branch_list.len(), 1);
    }

    #[test]
    fn merge_refresh_detects_branch_list_changes() {
        let mut prev = RepositorySnapshot {
            branch: Some("master".into()),
            head: Some("old".into()),
            last_commit_message: None,
            has_remote: false,
            ahead: 0,
            behind: 0,
            branch_list: vec![Branch {
                name: "master".into(),
                is_head: true,
            }],
            statuses_by_path: BTreeMap::new(),
        };
        let data = RefreshData {
            paths: vec![PathBuf::from(".git/HEAD")],
            head_queried: true,
            branch: Some("feature".into()),
            head: Some("new".into()),
            last_commit_message: None,
            has_remote: false,
            ahead: 0,
            behind: 0,
            // 外部 checkout 后分支列表重读：is_head 迁移到 feature。
            branches: vec![
                Branch {
                    name: "master".into(),
                    is_head: false,
                },
                Branch {
                    name: "feature".into(),
                    is_head: true,
                },
            ],
            statuses: zcv_git::GitStatus::default(),
            staged: HashMap::new(),
            unstaged: HashMap::new(),
            hunks: Vec::new(),
        };

        let (statuses_changed, head_changed) = merge_refresh(&mut prev, data);
        assert!(!statuses_changed);
        assert!(head_changed);
        assert_eq!(prev.branch_list.len(), 2);
        assert!(prev.branch_list[1].is_head);
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
}
