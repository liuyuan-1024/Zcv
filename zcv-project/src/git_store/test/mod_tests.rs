use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
#[cfg(unix)]
use std::time::{Duration, Instant};

use super::*;
use crate::test_support::{rev_parse, run_git, test_git_repo};

/// 测试用语言注册表；GitStore 的修订文档按它解析语言。
fn test_registry() -> Arc<LanguageRegistry> {
    Arc::new(LanguageRegistry::new())
}

use gpui::AppContext;
use zcv_buffer_diff::BufferDiffInput;
use zcv_git::StatusCode;

fn absolute(path: PathBuf) -> AbsolutePathBuf {
    AbsolutePathBuf::canonicalize(&path)
        .or_else(|_| AbsolutePathBuf::new(path))
        .expect("测试路径必须是绝对路径")
}

impl GitStore {
    fn status_for_directory(&self, path: &Path) -> Option<FileStatus> {
        self.status_index
            .status_for_directory(&canonicalize_path(path).expect("测试路径必须可归一化"))
    }
}

#[gpui::test]
fn scan_discovers_repository_and_reports_status(cx: &mut gpui::TestAppContext) {
    let (root, _temp) = test_git_repo();
    fs::write(root.join("tracked.txt"), "已修改\n").expect("应修改文件");

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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
    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
    cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
    cx.run_until_parked();
    let total = cx.read_entity(&git_store, |store, _| store.total_diff_stat());
    assert_eq!((total.added, total.deleted), (1, 1), "未暂存 1 增 1 删");

    // 暂存后再次修改：staged 与 unstaged 各计一份，合并计数。
    run_git(&root, &["add", "tracked.txt"]);
    std::fs::write(root.join("tracked.txt"), "第一行\n第二行（改）\n第三行\n").expect("应写入文件");
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

    let git_store = cx.update(|cx| {
        cx.new(|cx| GitStore::new(Some(temp_dir.path().to_path_buf()), test_registry(), cx))
    });
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
    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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
fn index_metadata_event_refreshes_the_repository(cx: &mut gpui::TestAppContext) {
    let (root, _temp) = test_git_repo();
    let path = root.join("tracked.txt");
    fs::write(&path, "暂存内容\n").expect("应修改文件");

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
    cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
    cx.run_until_parked();

    run_git(&root, &["add", "tracked.txt"]);
    cx.update_entity(&git_store, |store, cx| {
        store.refresh_statuses_for_paths(&[root.join(".git/index")], cx)
    });
    cx.run_until_parked();

    let status = cx.read_entity(&git_store, |store, _| {
        store.status_for_path(&path).map(|entry| entry.status)
    });
    assert!(status.is_some_and(|status| status.has_staged()));
    assert!(!status.is_some_and(|status| status.has_unstaged()));
}

#[gpui::test]
fn external_checkout_updates_head(cx: &mut gpui::TestAppContext) {
    let (root, _temp) = test_git_repo();
    // 第二个分支。
    run_git(&root, &["checkout", "-q", "-b", "feature"]);
    fs::write(root.join("tracked.txt"), "feature 内容\n").expect("应写入");
    run_git(&root, &["commit", "-q", "-am", "feature"]);

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
    cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
    cx.run_until_parked();

    // 外部 checkout 回 master：fs 事件同时触发 .git/HEAD 与工作区文件，
    // 增量刷新包含 .git 路径 → 重读 head（快路径只跳过纯文件变化批次）。
    run_git(&root, &["checkout", "-q", "master"]);
    cx.update_entity(&git_store, |store, cx| {
        store.refresh_statuses_for_paths(&[root.join("tracked.txt"), root.join(".git/HEAD")], cx)
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
    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
    cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
    cx.run_until_parked();

    // 修改工作区文件，HEAD 内容应仍是初始版本。
    fs::write(root.join("tracked.txt"), "已修改\n").expect("应修改文件");
    let path = root.join("tracked.txt");
    // 前台任务由测试调度器驱动（block 只跑后台任务，无法推进）。
    cx.read_entity(&git_store, |store, cx| {
        store.load_revision_document(GitRevision::Head, &path, cx)
    })
    .detach();
    cx.run_until_parked();
    // 加载结果已由 GitStore 自行回填缓存。
    let text = cx.read_entity(&git_store, |store, cx| {
        store.revision_text(GitRevision::Head, &path, cx)
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

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
    cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
    cx.run_until_parked();
    let path = root.join("tracked.txt");
    cx.read_entity(&git_store, |store, cx| {
        store.load_revision_document(GitRevision::Index, &path, cx)
    })
    .detach();
    cx.read_entity(&git_store, |store, cx| {
        store.load_revision_document(GitRevision::Index, &unchanged_path, cx)
    })
    .detach();
    cx.run_until_parked();

    let text = cx.read_entity(&git_store, |store, cx| {
        store.revision_text(GitRevision::Index, &path, cx)
    });
    assert_eq!(text.as_deref(), Some("已暂存内容\n"));

    // 状态类型与增删行统计保持不变时，index 内容变化仍必须被就地重读。
    fs::write(&path, "第二版暂存\n").expect("应更新暂存版本");
    run_git(&root, &["add", "tracked.txt"]);
    fs::write(&path, "第二版工作区\n").expect("应更新工作区版本");
    cx.update_entity(&git_store, |store, cx| {
        store.refresh_statuses_for_paths(std::slice::from_ref(&path), cx)
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&git_store, |store, cx| store.revision_text(
            GitRevision::Index,
            &path,
            cx
        ))
        .as_deref(),
        Some("第二版暂存\n"),
        "即使状态枚举与行数未变，刷新路径也必须就地重读 index 文本"
    );
    assert_eq!(
        cx.read_entity(&git_store, |store, cx| store.revision_text(
            GitRevision::Index,
            &unchanged_path,
            cx
        ))
        .as_deref(),
        Some("未变更内容\n"),
        "单路径刷新不应使其他文件的 index 文本失效"
    );

    cx.read_entity(&git_store, |store, cx| {
        store.load_revision_document(GitRevision::Index, &path, cx)
    })
    .detach();
    cx.run_until_parked();
    let text = cx.read_entity(&git_store, |store, cx| {
        store.revision_text(GitRevision::Index, &path, cx)
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

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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

    let git_store = cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx));
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
    let git_store = cx.new(|cx| GitStore::new(Some(root), test_registry(), cx));
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
    let git_store = cx.new(|cx| GitStore::new(None, test_registry(), cx));
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
fn cancelling_running_push_allows_clean_retry_after_process_exit(cx: &mut gpui::TestAppContext) {
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

    let git_store = cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx));
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

    let git_store = cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx));
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

    let git_store = cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx));
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
        store.stage_paths(vec![absolute(root.join("tracked.txt"))], cx);
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
    let staged_diff_stat = cx.read_entity(&git_store, |store, _| {
        store
            .status_for_path(&root.join("tracked.txt"))
            .map(|entry| entry.staged_diff_stat)
    });
    assert_eq!(
        staged_diff_stat,
        Some(DiffStat {
            added: 1,
            deleted: 2,
        }),
        "整文件暂存后应保留已暂存的增减行数"
    );
    assert!(
        cx.read_entity(&git_store, |store, _| store.has_staged_changes()),
        "存在已暂存改动时应具备提交资格"
    );

    // 取消暂存 → 回到未暂存。
    git_store.update(cx, |store, cx| {
        store.unstage_paths(vec![absolute(root.join("tracked.txt"))], cx);
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

    let git_store = cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx));
    git_store.update(cx, |store, cx| store.schedule_scan(cx));
    cx.run_until_parked();

    // 暂存整个 src 目录：修改 + 未跟踪 + 子目录文件一并进入 index。
    git_store.update(cx, |store, cx| {
        store.stage_paths(vec![absolute(root.join("src"))], cx);
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
        store.unstage_paths(vec![absolute(root.join("src"))], cx);
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

/// 同一 (working, base, index) 的 diff 实体在 GitStore 中按路径共享，缓存失效后才重建。
#[gpui::test]
fn file_diff_is_shared_by_working_base_and_index(cx: &mut gpui::TestAppContext) {
    let (root, _temp) = test_git_repo();
    fs::write(root.join("tracked.txt"), "第一行\n已修改\n").expect("应修改工作区文件");
    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
    cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
    cx.run_until_parked();
    let path = canonicalize_path(&root.join("tracked.txt")).expect("测试路径必须可归一化");
    let native_path = path.clone().into_path_buf();
    for revision in [GitRevision::Head, GitRevision::Index] {
        cx.read_entity(&git_store, |store, cx| {
            store.load_revision_document(revision, &path, cx)
        })
        .detach();
    }
    cx.run_until_parked();
    let working = cx.update(|cx| {
        let buffer = Buffer::from_text("第一行\n已修改\n".to_owned(), BufferConfig::default())
            .expect("应创建 Buffer");
        cx.new(|cx| {
            LanguageBuffer::new(
                buffer,
                Some(native_path.clone()),
                Arc::new(LanguageRegistry::new()),
                cx,
            )
        })
    });
    let spec = |store: &GitStore, cx: &gpui::App| BufferDiffInput {
        working: working.clone(),
        path: native_path.clone(),
        base_text: store.revision_text(GitRevision::Head, &path, cx),
        index_text: store.revision_text(GitRevision::Index, &path, cx),
        language_registry: store.language_registry(),
        key: 0,
        operations: None,
    };
    let first = git_store.update(cx, |store, cx| {
        let input = spec(store, cx);
        store.file_diff(&input, GitRevision::Head, GitRevision::Index, cx)
    });
    let second = git_store.update(cx, |store, cx| {
        let input = spec(store, cx);
        store.file_diff(&input, GitRevision::Head, GitRevision::Index, cx)
    });
    assert_eq!(
        first.entity_id(),
        second.entity_id(),
        "同一 (working, base, index) 应复用同一 diff 实体"
    );

    // index 文本变化：必须在同一实体上增量安装，而不是丢弃缓存另建实体。
    run_git(&root, &["add", "tracked.txt"]);
    cx.update_entity(&git_store, |store, cx| {
        store.refresh_statuses_for_paths(std::slice::from_ref(&native_path), cx)
    });
    cx.run_until_parked();
    let third = git_store.update(cx, |store, cx| {
        let input = spec(store, cx);
        store.file_diff(&input, GitRevision::Head, GitRevision::Index, cx)
    });
    assert_eq!(
        first.entity_id(),
        third.entity_id(),
        "修订文本变化必须复用同一 diff 实体"
    );
    assert_eq!(
        first.read_with(cx, |diff, cx| {
            diff.index_source()
                .map(|source| snapshot_text(&source.read(cx).text_snapshot()))
        }),
        Some("第一行\n已修改\n".to_owned()),
        "index 参照文本必须就地在原实体上推进"
    );
}

/// 变更块操作：界面线程从 diff 快照生成确定编辑并立即写入 optimistic pending，后台只应用该编辑写入 index。
#[gpui::test]
fn diff_operations_stage_hunk_writes_index_and_keeps_pending(cx: &mut gpui::TestAppContext) {
    let (root, _temp) = test_git_repo();
    fs::write(root.join("tracked.txt"), "第一行\n已修改\n").expect("应修改工作区文件");
    // 无关文件用于验证：暂存一个路径不得让其他路径的 index 文本整体失效。
    fs::write(root.join("other.txt"), "无关文件\n").expect("应写入无关文件");
    run_git(&root, &["add", "other.txt"]);
    run_git(&root, &["commit", "-q", "-m", "add other"]);

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
    cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
    cx.run_until_parked();

    let path = canonicalize_path(&root.join("tracked.txt")).expect("测试路径必须可归一化");
    let native_path = path.clone().into_path_buf();
    let other_path = canonicalize_path(&root.join("other.txt")).expect("测试路径必须可归一化");
    cx.read_entity(&git_store, |store, cx| {
        store.load_revision_document(GitRevision::Index, &path, cx)
    })
    .detach();
    cx.read_entity(&git_store, |store, cx| {
        store.load_revision_document(GitRevision::Index, &other_path, cx)
    })
    .detach();
    cx.run_until_parked();
    let working = cx.update(|cx| {
        let buffer = Buffer::from_text("第一行\n已修改\n".to_owned(), BufferConfig::default())
            .expect("应创建 Buffer");
        cx.new(|cx| {
            LanguageBuffer::new(
                buffer,
                Some(native_path.clone()),
                Arc::new(LanguageRegistry::new()),
                cx,
            )
        })
    });
    let operations = git_store.read_with(cx, |store, _| store.diff_operations(GitRevision::Index));
    let diff = cx.update(|cx| {
        cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    working: working.clone(),
                    path: native_path.clone(),
                    base_text: Some("第一行\n第二行\n".to_owned()),
                    index_text: None,
                    language_registry: Arc::new(LanguageRegistry::new()),
                    key: 0,
                    operations: Some(operations),
                },
                cx,
            )
        })
    });
    cx.run_until_parked();

    let range = diff.read_with(cx, |diff, _| {
        assert_eq!(diff.snapshot().hunks().len(), 1);
        diff.snapshot().hunks()[0].buffer_range.clone()
    });

    // 操作发起后立即抑制该 hunk；乐观 index 批次只在后台写入并经权威扫描确认后前进。
    cx.update(|cx| {
        let operations = diff.read(cx).operations().expect("应有操作实现");
        operations.stage(diff.clone(), vec![range], cx);
    });
    diff.read_with(cx, |diff, _| {
        assert!(
            diff.snapshot().visible_hunks().is_empty(),
            "pending 应立即抑制 hunk"
        );
        assert_eq!(diff.snapshot().pending_hunks().len(), 1);
    });
    assert_eq!(
        cx.read_entity(&git_store, |store, cx| {
            store.revision_text(GitRevision::Index, &path, cx)
        })
        .as_deref(),
        Some("第一行\n第二行\n"),
        "暂存结果只在权威扫描确认后前进，不提前改写 index 文本"
    );

    cx.run_until_parked();
    // 权威扫描就地重读 index 文本；写入结果不提前，也不靠丢弃文档来触发重载。
    diff.read_with(cx, |diff, _| {
        assert_eq!(diff.snapshot().pending_hunks().len(), 1);
    });
    assert_eq!(
        cx.read_entity(&git_store, |store, cx| store.revision_text(
            GitRevision::Index,
            &path,
            cx
        ))
        .as_deref(),
        Some("第一行\n已修改\n"),
        "权威扫描必须就地刷新 index 文本"
    );
    let repository = zcv_git::RealGitRepository::open(&root.join(".git")).expect("应打开工作仓库");
    let index = repository
        .load_revisions(&[":tracked.txt"])
        .expect("应读取 index")
        .pop()
        .flatten()
        .expect("index 应包含文件");
    assert_eq!(
        String::from_utf8(index).expect("index 应为 UTF-8"),
        "第一行\n已修改\n",
        "后台必须应用确定的编辑结果"
    );
    assert!(
        cx.read_entity(&git_store, |store, _| store
            .revision_document_loaded(GitRevision::Index, &other_path)),
        "未变化路径的 index 文本不得因另一路径暂存而被整体失效"
    );
}

/// 工作区版本在 diff 快照之后前进时，操作锚点必须在当前快照上重新解析，
/// 而不是因版本不相等丢弃并提示用户刷新。
#[gpui::test]
fn staging_resolves_hunk_anchors_on_the_current_working_snapshot(cx: &mut gpui::TestAppContext) {
    let (root, _temp) = test_git_repo();
    fs::write(root.join("tracked.txt"), "第一行\n已修改\n").expect("应修改工作区文件");

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
    cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
    cx.run_until_parked();

    let path = canonicalize_path(&root.join("tracked.txt")).expect("测试路径必须可归一化");
    let native_path = path.clone().into_path_buf();
    cx.read_entity(&git_store, |store, cx| {
        store.load_revision_document(GitRevision::Index, &path, cx)
    })
    .detach();
    cx.run_until_parked();
    let working = cx.update(|cx| {
        let buffer = Buffer::from_text("第一行\n已修改\n".to_owned(), BufferConfig::default())
            .expect("应创建 Buffer");
        cx.new(|cx| {
            LanguageBuffer::new(
                buffer,
                Some(native_path.clone()),
                Arc::new(LanguageRegistry::new()),
                cx,
            )
        })
    });
    let operations = git_store.read_with(cx, |store, _| store.diff_operations(GitRevision::Index));
    let diff = cx.update(|cx| {
        cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    working: working.clone(),
                    path: native_path.clone(),
                    base_text: Some("第一行\n第二行\n".to_owned()),
                    index_text: None,
                    language_registry: Arc::new(LanguageRegistry::new()),
                    key: 0,
                    operations: Some(operations),
                },
                cx,
            )
        })
    });
    cx.run_until_parked();
    let range = diff.read_with(cx, |diff, _| {
        assert_eq!(diff.snapshot().hunks().len(), 1);
        diff.snapshot().hunks()[0].buffer_range.clone()
    });

    // 快照之后工作区版本前进：hunk 之后追加一行。
    working.update(cx, |working, cx| {
        working
            .replace_text("第一行\n已修改\n新增行\n".to_owned(), cx)
            .expect("应推进工作区文本版本");
    });
    let working_version = working.read_with(cx, |working, _| working.text_snapshot().version());
    assert_ne!(
        range.start.version(),
        working_version,
        "测试前提：操作锚点版本已落后于工作区"
    );
    cx.update(|cx| {
        let operations = diff.read(cx).operations().expect("应有操作实现");
        operations.stage(diff.clone(), vec![range], cx);
    });
    diff.read_with(cx, |diff, _| {
        assert_eq!(
            diff.snapshot().pending_hunks().len(),
            1,
            "锚点落后必须在当前快照重新解析，不得丢弃操作"
        );
    });
    cx.run_until_parked();
    let repository = zcv_git::RealGitRepository::open(&root.join(".git")).expect("应打开工作仓库");
    let index = repository
        .load_revisions(&[":tracked.txt"])
        .expect("应读取 index")
        .pop()
        .flatten()
        .expect("index 应包含文件");
    assert_eq!(
        String::from_utf8(index).expect("index 应为 UTF-8"),
        "第一行\n已修改\n",
        "落后锚点必须按当前快照生成编辑"
    );
}

/// 同文件连续暂存两个 hunk：不等待后台写入完成，编辑必须合并进同一乐观批次，
/// 而不是被在途互斥拒绝。
#[gpui::test]
fn staging_two_hunks_without_waiting_merges_pending_edits(cx: &mut gpui::TestAppContext) {
    let (root, _temp) = test_git_repo();
    fs::write(root.join("two.txt"), "a0\na1\na2\na3\na4\na5\n").expect("应写入初始文件");
    run_git(&root, &["add", "two.txt"]);
    run_git(&root, &["commit", "-q", "-m", "two"]);
    fs::write(root.join("two.txt"), "A0\na1\na2\na3\nA4\na5\n").expect("应修改两处");

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
    cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
    cx.run_until_parked();

    let path = canonicalize_path(&root.join("two.txt")).expect("测试路径必须可归一化");
    let native_path = path.clone().into_path_buf();
    cx.read_entity(&git_store, |store, cx| {
        store.load_revision_document(GitRevision::Index, &path, cx)
    })
    .detach();
    cx.run_until_parked();
    let working = cx.update(|cx| {
        let buffer = Buffer::from_text(
            "A0\na1\na2\na3\nA4\na5\n".to_owned(),
            BufferConfig::default(),
        )
        .expect("应创建 Buffer");
        cx.new(|cx| {
            LanguageBuffer::new(
                buffer,
                Some(native_path.clone()),
                Arc::new(LanguageRegistry::new()),
                cx,
            )
        })
    });
    let operations = git_store.read_with(cx, |store, _| store.diff_operations(GitRevision::Index));
    let diff = cx.update(|cx| {
        cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    working: working.clone(),
                    path: native_path.clone(),
                    base_text: Some("a0\na1\na2\na3\na4\na5\n".to_owned()),
                    index_text: None,
                    language_registry: Arc::new(LanguageRegistry::new()),
                    key: 0,
                    operations: Some(operations),
                },
                cx,
            )
        })
    });
    cx.run_until_parked();
    let ranges = diff.read_with(cx, |diff, _| {
        let hunks = diff.snapshot().hunks().to_vec();
        assert_eq!(hunks.len(), 2, "应有两个 hunk");
        hunks
            .iter()
            .map(|hunk| hunk.buffer_range.clone())
            .collect::<Vec<_>>()
    });

    // 两笔连续发起，不等待第一笔后台写入。
    cx.update(|cx| {
        let operations = diff.read(cx).operations().expect("应有操作实现");
        operations.stage(diff.clone(), vec![ranges[0].clone()], cx);
        operations.stage(diff.clone(), vec![ranges[1].clone()], cx);
    });
    diff.read_with(cx, |diff, _| {
        assert_eq!(
            diff.snapshot().pending_hunks().len(),
            2,
            "同文件两笔操作都必须写入 pending，不因在途而被拒绝"
        );
    });
    cx.run_until_parked();
    let repository = zcv_git::RealGitRepository::open(&root.join(".git")).expect("应打开工作仓库");
    let index = repository
        .load_revisions(&[":two.txt"])
        .expect("应读取 index")
        .pop()
        .flatten()
        .expect("index 应包含文件");
    assert_eq!(
        String::from_utf8(index).expect("index 应为 UTF-8"),
        "A0\na1\na2\na3\nA4\na5\n",
        "两笔编辑必须合并落盘"
    );
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

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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
    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
    cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
    cx.run_until_parked();

    let state = cx.read_entity(&git_store, |store, _| store.remote_operation_state());
    assert_eq!(state, RemoteOperationState::default());
}

#[gpui::test]
fn scan_reports_branch_list(cx: &mut gpui::TestAppContext) {
    let (root, _temp) = test_git_repo();
    run_git(&root, &["checkout", "-q", "-b", "feature"]);

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
    cx.update_entity(&git_store, |store, cx| store.schedule_scan(cx));
    cx.run_until_parked();

    // 选择器确认切换到 master：job 完成后自动重扫，Head 事件驱动 UI 刷新。
    cx.update_entity(&git_store, |store, cx| {
        let _task = store.checkout_branch_with_result("master".into(), cx);
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
    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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

    let git_store =
        cx.update(|cx| cx.new(|cx| GitStore::new(Some(root.clone()), test_registry(), cx)));
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
    let git_store = cx.update(|cx| {
        cx.new(|cx| GitStore::new(Some(temp_dir.path().to_path_buf()), test_registry(), cx))
    });
    cx.update_entity(&git_store, |store, cx| {
        let _task = store.checkout_branch_with_result("master".into(), cx);
        store.create_branch("feature".into(), cx);
    });
    cx.run_until_parked();
}
