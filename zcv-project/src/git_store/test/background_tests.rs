use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use zcv_git::{DiffStat, FileStatus, RealGitRepository};

use super::*;
use crate::git_store::{RepositorySnapshot, StatusEntry};
use crate::test_support::{run_git, test_git_repo};

fn relative(path: &str) -> RelativePathBuf {
    RelativePathBuf::from_path(std::path::Path::new(path)).expect("测试路径应为有效的仓库相对路径")
}

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
                relative("a.txt"),
                StatusEntry {
                    status: FileStatus::Untracked,
                    diff_stat: DiffStat::default(),
                    staged_diff_stat: DiffStat::default(),
                    unstaged_diff_stat: DiffStat::default(),
                },
            ),
            (
                relative("sub/b.txt"),
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
        paths: vec![relative("a.txt"), relative("sub")],
        head_queried: true,
        branch: Some("master".into()),
        head: Some("old".into()),
        last_commit_message: None,
        has_remote: true,
        ahead: 1,
        behind: 0,
        branches: Vec::new(),
        // a.txt 变干净（无输出 → 移除）；sub/c.txt 新增。
        statuses: GitStatus {
            statuses: vec![(PathBuf::from("sub/c.txt"), FileStatus::Untracked)],
            branch: None,
        },
        staged: HashMap::new(),
        unstaged: HashMap::new(),
        clean_conflicts: Vec::new(),
    };

    let mut prev = prev;
    let (statuses_changed, head_changed) = merge_refresh(&mut prev, data);
    assert!(statuses_changed);
    assert!(!head_changed);
    assert!(!prev.statuses_by_path.contains_key(&relative("a.txt")));
    assert!(!prev.statuses_by_path.contains_key(&relative("sub/b.txt")));
    assert!(prev.statuses_by_path.contains_key(&relative("sub/c.txt")));
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
        paths: vec![relative("a.txt")],
        head_queried: true,
        branch: Some("master".into()),
        head: Some("new".into()),
        last_commit_message: None,
        has_remote: false,
        ahead: 0,
        behind: 0,
        branches: Vec::new(),
        statuses: GitStatus::default(),
        staged: HashMap::new(),
        unstaged: HashMap::new(),
        clean_conflicts: Vec::new(),
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
        paths: vec![relative("a.txt")],
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
        statuses: GitStatus::default(),
        staged: HashMap::new(),
        unstaged: HashMap::new(),
        clean_conflicts: Vec::new(),
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
        paths: vec![relative(".git/HEAD")],
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
        statuses: GitStatus::default(),
        staged: HashMap::new(),
        unstaged: HashMap::new(),
        clean_conflicts: Vec::new(),
    };

    let (statuses_changed, head_changed) = merge_refresh(&mut prev, data);
    assert!(!statuses_changed);
    assert!(head_changed);
    assert_eq!(prev.branch_list.len(), 2);
    assert!(prev.branch_list[1].is_head);
}

#[test]
fn cancelled_push_reconciliation_distinguishes_completed_and_not_pushed() {
    let (root, _worktree_temp) = test_git_repo();
    let remote_temp = tempfile::tempdir().expect("应创建远端临时目录");
    let remote = remote_temp.path().join("remote.git");
    run_git(
        remote_temp.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    run_git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    run_git(&root, &["push", "-q", "-u", "origin", "master"]);

    let repository: Arc<dyn GitRepository> =
        Arc::new(RealGitRepository::open(&root.join(".git")).expect("应打开工作仓库"));
    assert_eq!(
        reconcile_cancelled_operation(GitOperationKind::Push, Some(repository.clone())),
        GitOperationOutcome::CompletedBeforeCancellation,
        "远端已包含本地提交时应识别为取消前完成"
    );

    std::fs::write(root.join("not-pushed.txt"), "尚未推送\n").expect("应写入新文件");
    run_git(&root, &["add", "not-pushed.txt"]);
    run_git(&root, &["commit", "-q", "-m", "尚未推送"]);
    assert_eq!(
        reconcile_cancelled_operation(GitOperationKind::Push, Some(repository)),
        GitOperationOutcome::Cancelled,
        "本地仍领先远端时应确认推送已取消"
    );
}
