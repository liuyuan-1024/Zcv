use super::*;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(unix)]
use std::time::{Duration, Instant};

use tempfile::TempDir;

use crate::FileStatus;
use crate::diff::DiffHunkKind::{Added, Deleted, Modified};
use crate::status::StatusCode;

/// 创建带一个初始提交的临时 git 仓库，返回 (仓库根, 目录句柄)。
///
/// `-b master` 固定初始分支名，避免依赖本机 git 的 `init.defaultBranch` 配置。
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

fn run_in(dir: &Path, args: &[&str]) -> Output {
    let output = Command::new(args[0])
        .args(&args[1..])
        .current_dir(dir)
        .output()
        .unwrap_or_else(|error| {
            panic!("命令 {:?} 在 {:?} 执行失败：{error}", args, dir);
        });
    assert!(
        output.status.success(),
        "命令 {:?} 失败：{}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn open_repo(root: &Path) -> RealGitRepository {
    RealGitRepository::open(&root.join(".git")).expect("open 应成功")
}

#[test]
fn status_reports_all_states() {
    let (root, _temp) = test_repo();
    let repo = open_repo(&root);

    fs::write(root.join("tracked.txt"), "已修改\n").expect("应修改文件");
    fs::write(root.join("new.txt"), "新文件\n").expect("应新建文件");
    fs::remove_file(root.join("tracked.txt")).ok();
    fs::write(root.join("tracked.txt"), "已修改\n").expect("应重新写入");

    let status = repo.status(&[]).expect("status 应成功");
    let by_path: HashMap<_, _> = status
        .statuses
        .iter()
        .map(|(p, s)| (p.as_path(), s))
        .collect();

    assert!(
        by_path
            .get(Path::new("tracked.txt"))
            .expect("应有 tracked.txt")
            .is_modified()
    );
    assert!(
        by_path
            .get(Path::new("new.txt"))
            .expect("应有 new.txt")
            .is_untracked()
    );
}

#[test]
fn status_accepts_path_prefixes() {
    let (root, _temp) = test_repo();
    let repo = open_repo(&root);
    fs::create_dir_all(root.join("src")).expect("应创建目录");
    fs::write(root.join("src/one.rs"), "1\n").expect("应创建文件");
    fs::write(root.join("two.rs"), "2\n").expect("应创建文件");

    let status = repo.status(&[PathBuf::from("src")]).expect("status 应成功");
    let paths: Vec<_> = status.statuses.iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(paths, vec![PathBuf::from("src/one.rs")]);
}

#[test]
fn diff_stat_reports_staged_and_unstaged() {
    let (root, _temp) = test_repo();
    let repo = open_repo(&root);

    fs::write(root.join("tracked.txt"), "第一行\n第二行\n第三行\n").expect("应修改文件");
    run_in(&root, &["git", "add", "tracked.txt"]);
    fs::write(root.join("tracked.txt"), "第一行\n改动\n").expect("应再次修改");

    let staged = repo.diff_stat(true, &[]).expect("diff_stat 应成功");
    assert_eq!(
        staged.get(Path::new("tracked.txt")),
        Some(&DiffStat {
            added: 1,
            deleted: 0
        })
    );
    // 工作区相对 index："第二行"→"改动"（+1/-1）且"第三行"被删（-1）。
    let unstaged = repo.diff_stat(false, &[]).expect("diff_stat 应成功");
    assert_eq!(
        unstaged.get(Path::new("tracked.txt")),
        Some(&DiffStat {
            added: 1,
            deleted: 2
        })
    );
}

#[test]
fn diff_stat_with_path_prefix() {
    let (root, _temp) = test_repo();
    let repo = open_repo(&root);
    // diff 只覆盖已跟踪文件，未跟踪文件先 add。
    fs::write(root.join("a.txt"), "x\n").expect("应创建文件");
    fs::write(root.join("b.txt"), "y\n").expect("应创建文件");
    run_in(&root, &["git", "add", "a.txt", "b.txt"]);
    fs::write(root.join("a.txt"), "x\nx\n").expect("应修改文件");

    let stats = repo
        .diff_stat(false, &[PathBuf::from("a.txt")])
        .expect("diff_stat 应成功");
    assert!(stats.contains_key(Path::new("a.txt")));
    assert!(!stats.contains_key(Path::new("b.txt")));
}

#[test]
fn load_revisions_reads_head_and_index_contents() {
    let (root, _temp) = test_repo();
    let repo = open_repo(&root);

    let (head_oid, _) = repo.head_commit().expect("head_commit 应成功");
    let head_oid = head_oid.expect("应有 HEAD");

    let contents = repo
        .load_revisions(&[
            "HEAD:tracked.txt",
            ":tracked.txt",
            &format!("{head_oid}:tracked.txt"),
            "HEAD:missing.txt",
        ])
        .expect("load_revisions 应成功");

    let expected = "第一行\n第二行\n".as_bytes();
    assert_eq!(contents[0].as_deref(), Some(expected));
    assert_eq!(contents[1].as_deref(), Some(expected));
    assert_eq!(contents[2].as_deref(), Some(expected));
    assert_eq!(contents[3], None);
}

#[test]
fn status_does_not_touch_index_mtime() {
    // --no-optional-locks 生效：扫描不应回写 index（racy-git 写回），
    // 否则会产生"扫描 → fs 事件 → 再扫描"的自触发循环。
    let (root, _temp) = test_repo();
    let repo = open_repo(&root);
    let index_path = root.join(".git/index");
    fs::write(root.join("tracked.txt"), "修改\n").expect("应修改文件");

    let before = fs::metadata(&index_path)
        .expect("index 应存在")
        .modified()
        .expect("mtime");
    repo.status(&[]).expect("status 应成功");
    let after = fs::metadata(&index_path)
        .expect("index 应存在")
        .modified()
        .expect("mtime");

    assert_eq!(before, after, "status 不应回写 index");
}

#[test]
fn status_reports_ignored_entries_with_ignored_matching() {
    // --ignored=matching 语义：被忽略目录目录级输出（`!! dir/`），
    // 未被目录覆盖的忽略文件逐条输出（含 .git/info/exclude 命中），
    // 未跟踪文件仍逐条输出。
    let (root, _temp) = test_repo();
    let repo = open_repo(&root);

    fs::write(root.join(".gitignore"), "node_modules/\nignored.log\n").expect("应写入 .gitignore");
    fs::create_dir_all(root.join("node_modules/pkg")).expect("应创建目录");
    fs::write(root.join("node_modules/pkg/index.js"), "x\n").expect("应创建文件");
    fs::write(root.join("ignored.log"), "y\n").expect("应创建文件");
    fs::write(root.join("new.txt"), "z\n").expect("应创建文件");
    fs::write(root.join("tracked.txt"), "修改\n").expect("应修改文件");

    let status = repo.status(&[]).expect("status 应成功");
    let by_path: HashMap<_, _> = status
        .statuses
        .iter()
        .map(|(p, s)| (p.as_path(), s))
        .collect();

    assert!(
        by_path
            .get(Path::new("node_modules"))
            .expect("忽略目录应有条目")
            .is_ignored()
    );
    assert!(
        by_path
            .get(Path::new("ignored.log"))
            .expect("忽略文件应有条目")
            .is_ignored()
    );
    assert!(
        by_path
            .get(Path::new("new.txt"))
            .expect("未跟踪文件应有条目")
            .is_untracked()
    );
    assert!(
        by_path
            .get(Path::new("tracked.txt"))
            .expect("已跟踪修改应有条目")
            .is_modified()
    );
    // 忽略目录内部不逐文件展开。
    assert!(!by_path.contains_key(Path::new("node_modules/pkg/index.js")));
}

#[test]
fn status_reports_info_exclude_ignores() {
    // .git/info/exclude 是仓库级忽略，自解析 .gitignore 覆盖不到。
    let (root, _temp) = test_repo();
    let repo = open_repo(&root);
    fs::write(root.join(".git/info/exclude"), "secret.log\n").expect("应写入 exclude");
    fs::write(root.join("secret.log"), "x\n").expect("应创建文件");

    let status = repo.status(&[]).expect("status 应成功");
    let by_path: HashMap<_, _> = status
        .statuses
        .iter()
        .map(|(p, s)| (p.as_path(), s))
        .collect();
    assert!(
        by_path
            .get(Path::new("secret.log"))
            .expect("info/exclude 命中应有条目")
            .is_ignored()
    );
}

#[test]
fn status_with_path_prefix_reports_file_level_ignored() {
    // 增量刷新传具体文件路径时，ignored 按文件级输出。
    let (root, _temp) = test_repo();
    let repo = open_repo(&root);
    fs::write(root.join(".gitignore"), "ignored.log\n").expect("应写入 .gitignore");
    fs::write(root.join("ignored.log"), "y\n").expect("应创建文件");

    let status = repo
        .status(&[PathBuf::from("ignored.log")])
        .expect("status 应成功");
    let by_path: HashMap<_, _> = status
        .statuses
        .iter()
        .map(|(p, s)| (p.as_path(), s))
        .collect();
    assert!(
        by_path
            .get(Path::new("ignored.log"))
            .expect("路径参数下忽略文件应有条目")
            .is_ignored()
    );
}

/// 创建「本地裸远程 + 已推送初始提交的工作仓库」对，返回 (工作仓库根, 裸远程根, 临时目录句柄)。
///
/// 工作仓库与裸远程共用同一个 temp_dir，保证返回后目录仍存活。
fn test_repo_with_remote() -> (PathBuf, PathBuf, TempDir) {
    let temp_dir = tempfile::tempdir().expect("应创建临时目录");
    let remote = temp_dir.path().join("remote.git");
    run_in(
        temp_dir.path(),
        &["git", "init", "-q", "--bare", remote.to_str().unwrap()],
    );
    let root = temp_dir.path().join("work");
    std::fs::create_dir(&root).expect("应创建工作仓库目录");
    run_in(&root, &["git", "init", "-q", "-b", "master"]);
    run_in(&root, &["git", "config", "user.email", "test@example.com"]);
    run_in(&root, &["git", "config", "user.name", "Test User"]);
    std::fs::write(root.join("tracked.txt"), "第一行\n第二行\n").expect("应写入初始文件");
    run_in(&root, &["git", "add", "tracked.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
    run_in(
        &root,
        &["git", "remote", "add", "origin", remote.to_str().unwrap()],
    );
    run_in(&root, &["git", "push", "-q", "-u", "origin", "master"]);
    (root, remote, temp_dir)
}

#[test]
fn fetch_pull_push_work_against_remote() {
    let (root, remote, _temp) = test_repo_with_remote();
    let repo = open_repo(&root);

    // 制造"本地领先远程一个提交"：提交后 push，再回退本地 HEAD。
    std::fs::write(root.join("pushed.txt"), "推送内容\n").expect("应写入文件");
    run_in(&root, &["git", "add", "pushed.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "待推送提交"]);
    run_in(&root, &["git", "push", "-q", "origin", "master"]);
    run_in(&root, &["git", "reset", "-q", "--hard", "HEAD~1"]);

    // fetch：本地引用更新为远程状态，工作树不动。
    repo.fetch_cancellable(&GitCancellation::new())
        .expect("fetch 应成功");
    let ahead = run_in(
        &root,
        &["git", "rev-list", "--count", "HEAD..origin/master"],
    );
    assert_eq!(
        String::from_utf8_lossy(&ahead.stdout).trim(),
        "1",
        "fetch 后本地应落后远程一个提交"
    );

    // pull：合并远程提交，本地追上远程。
    repo.pull_cancellable(&GitCancellation::new())
        .expect("pull 应成功");
    let behind = run_in(
        &root,
        &["git", "rev-list", "--count", "HEAD..origin/master"],
    );
    assert_eq!(
        String::from_utf8_lossy(&behind.stdout).trim(),
        "0",
        "pull 后本地应追上远程"
    );
    assert!(
        root.join("pushed.txt").exists(),
        "pull 应带下远程提交的文件"
    );

    // push：把新提交推送到远程。
    std::fs::write(root.join("again.txt"), "再推一次\n").expect("应写入文件");
    run_in(&root, &["git", "add", "again.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "再推一次"]);
    repo.push_cancellable(&GitCancellation::new())
        .expect("push 应成功");
    // 从远程裸仓库验证提交已到达（bare 仓库 HEAD 指向 master）。
    let remote_head = run_in(&remote, &["git", "rev-parse", "master"]);
    let local_head = run_in(&root, &["git", "rev-parse", "HEAD"]);
    assert_eq!(
        String::from_utf8_lossy(&remote_head.stdout).trim(),
        String::from_utf8_lossy(&local_head.stdout).trim(),
        "push 后远程应指向本地 HEAD"
    );
}

#[cfg(unix)]
#[test]
fn cancelling_push_terminates_git_and_hook_process_tree() {
    let (root, _remote, _temp) = test_repo_with_remote();
    fs::write(root.join("cancel.txt"), "等待取消\n").expect("应写入待推送文件");
    run_in(&root, &["git", "add", "cancel.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "测试取消推送"]);

    let pid_path = root.join("hook-child.pid");
    let hook_path = root.join(".git/hooks/pre-push");
    // 进度行必须先于 PID 文件写入：取消线程见到 PID 文件即触发 SIGINT，若进度行在其后书写，高负载下钩子可能在两行之间被抢占，导致进度丢失。
    fs::write(
        &hook_path,
        format!(
            "#!/bin/sh\necho '正在等待测试钩子' >&2\nsleep 30 &\nchild=$!\necho $child > '{}'\nwait $child\n",
            pid_path.display()
        ),
    )
    .expect("应写入 pre-push 钩子");
    let mut permissions = fs::metadata(&hook_path)
        .expect("应读取钩子权限")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook_path, permissions).expect("应设置钩子可执行权限");

    let cancellation = GitCancellation::new();
    let cancel_from_thread = cancellation.clone();
    let child_pid = Arc::new(AtomicU32::new(0));
    let child_pid_from_thread = Arc::clone(&child_pid);
    let pid_path_from_thread = pid_path.clone();
    let cancel_thread = std::thread::spawn(move || {
        for _ in 0..200 {
            if let Ok(pid) = fs::read_to_string(&pid_path_from_thread)
                && let Ok(pid) = pid.trim().parse::<u32>()
            {
                child_pid_from_thread.store(pid, Ordering::Release);
                cancel_from_thread.cancel();
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("pre-push 子进程未在时限内启动");
    });

    let started = Instant::now();
    let result = open_repo(&root).push_cancellable(&cancellation);
    cancel_thread.join().expect("取消线程不应异常");
    assert!(result.is_err(), "取消后的 push 应返回错误");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "取消不应长时间阻塞"
    );
    assert!(
        cancellation
            .progress()
            .is_some_and(|progress| progress.contains("正在等待测试钩子")),
        "应保留 git 最近一行进度"
    );

    let pid = child_pid.load(Ordering::Acquire);
    for _ in 0..100 {
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        if !alive {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("取消 push 后钩子子进程仍然存活");
}

#[test]
fn has_remote_reports_true_and_false() {
    let (root, _temp) = test_repo();
    assert!(!open_repo(&root).has_remote().expect("has_remote 应成功"));

    let (root, _remote, _temp) = test_repo_with_remote();
    assert!(open_repo(&root).has_remote().expect("has_remote 应成功"));
}

#[test]
fn status_reports_branch_tracking() {
    let (root, _remote, _temp) = test_repo_with_remote();
    let repo = open_repo(&root);

    // 与远程同步：无 ahead/behind。
    let branch = repo
        .status(&[])
        .expect("status 应成功")
        .branch
        .expect("应有头行");
    assert_eq!(branch.upstream.as_deref(), Some("origin/master"));
    assert_eq!((branch.ahead, branch.behind), (0, 0));

    // 本地新提交 → ahead 1（可推送）。
    std::fs::write(root.join("new.txt"), "新提交\n").expect("应写入文件");
    run_in(&root, &["git", "add", "new.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "本地提交"]);
    let branch = repo
        .status(&[])
        .expect("status 应成功")
        .branch
        .expect("应有头行");
    assert_eq!((branch.ahead, branch.behind), (1, 0));

    // push 后回到同步（徽标消失的依据）。
    repo.push_cancellable(&GitCancellation::new())
        .expect("push 应成功");
    let branch = repo
        .status(&[])
        .expect("status 应成功")
        .branch
        .expect("应有头行");
    assert_eq!((branch.ahead, branch.behind), (0, 0));
}

/// 批量查询单路径的 hunks（路径不在结果中视为空）。
fn hunks_for(repository: &RealGitRepository, path: &Path) -> Vec<DiffHunk> {
    repository
        .diff_hunks_for_paths(DiffBase::Head, &[path.to_path_buf()])
        .expect("diff_hunks_for_paths 应成功")
        .into_iter()
        .find_map(|(parsed, hunks)| (parsed == path).then_some(hunks))
        .unwrap_or_default()
}

#[test]
fn diff_hunks_reports_worktree_changes() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);
    let tracked = Path::new("tracked.txt");

    // 干净文件：无 hunks。
    assert_eq!(hunks_for(&repository, tracked), vec![]);

    // 修改第 2 行 → Modified（range 1..2）。
    fs::write(root.join("tracked.txt"), "第一行\n改了第二行\n").expect("应写入文件");
    assert_eq!(
        hunks_for(&repository, tracked),
        vec![DiffHunk {
            range: 1..2,
            old_range: 1..2,
            kind: Modified,
        }]
    );

    // 末尾追加 → Added。
    fs::write(root.join("tracked.txt"), "第一行\n第二行\n新增行\n").expect("应写入文件");
    assert_eq!(
        hunks_for(&repository, tracked),
        vec![DiffHunk {
            range: 2..3,
            old_range: 1..1,
            kind: Added,
        }]
    );

    // 删除第一行 → Deleted（锚定删除点行 0）。
    fs::write(root.join("tracked.txt"), "第二行\n").expect("应写入文件");
    assert_eq!(
        hunks_for(&repository, tracked),
        vec![DiffHunk {
            range: 0..0,
            old_range: 0..1,
            kind: Deleted,
        }]
    );
}

#[test]
fn diff_hunks_empty_for_untracked_clean_and_binary() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);

    // 未跟踪文件：git diff HEAD 无输出。
    fs::write(root.join("untracked.txt"), "新的\n").expect("应写入文件");
    assert_eq!(hunks_for(&repository, Path::new("untracked.txt")), vec![]);

    // 干净文件：无 hunks。
    assert_eq!(hunks_for(&repository, Path::new("tracked.txt")), vec![]);

    // 已跟踪二进制文件：Binary files differ → 空。
    fs::write(root.join("img.png"), [0x89u8, 0x50, 0x4e, 0x47]).expect("应写入文件");
    run_in(&root, &["git", "add", "img.png"]);
    run_in(&root, &["git", "commit", "-q", "-m", "add png"]);
    fs::write(root.join("img.png"), [0x89u8, 0x50, 0x4e, 0x47, 0x00]).expect("应写入文件");
    assert_eq!(hunks_for(&repository, Path::new("img.png")), vec![]);
}

#[test]
fn diff_hunks_empty_without_head() {
    let temp_dir = tempfile::tempdir().expect("应创建临时目录");
    let root = temp_dir.path().to_path_buf();
    run_in(&root, &["git", "init", "-q", "-b", "master"]);

    let repository = open_repo(&root);
    assert_eq!(hunks_for(&repository, Path::new("tracked.txt")), vec![]);
}

#[test]
fn diff_hunks_batch_maps_results_to_paths() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);
    fs::write(root.join("a.txt"), "a\n").expect("应创建文件");
    fs::write(root.join("b.txt"), "b\n").expect("应创建文件");
    run_in(&root, &["git", "add", "a.txt", "b.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "add both"]);
    fs::write(root.join("a.txt"), "a\na\n").expect("应修改文件");
    fs::write(root.join("b.txt"), "b\nb\n").expect("应修改文件");

    let results = repository
        .diff_hunks_for_paths(
            DiffBase::Head,
            &[PathBuf::from("a.txt"), PathBuf::from("b.txt")],
        )
        .expect("批量 diff 应成功");
    let paths: Vec<_> = results.iter().map(|(path, _)| path.as_path()).collect();
    assert_eq!(paths, [Path::new("a.txt"), Path::new("b.txt")]);
    assert!(results.iter().all(|(_, hunks)| !hunks.is_empty()));
    // 未请求的路径不在结果中。
    assert!(
        results
            .iter()
            .all(|(path, _)| path.as_os_str() != "tracked.txt")
    );
}

#[test]
fn diff_hunks_separate_staged_and_unstaged_changes() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);
    let tracked = PathBuf::from("tracked.txt");

    fs::write(root.join(&tracked), "第一行\n已暂存修改\n").expect("应写入已暂存版本");
    run_in(&root, &["git", "add", "tracked.txt"]);
    fs::write(root.join(&tracked), "未暂存修改\n已暂存修改\n").expect("应写入工作区版本");

    let staged = repository
        .diff_hunks_for_paths(DiffBase::Staged, std::slice::from_ref(&tracked))
        .expect("应读取已暂存差异")
        .pop()
        .expect("应返回已暂存文件")
        .1;
    let unstaged = repository
        .diff_hunks_for_paths(DiffBase::Index, std::slice::from_ref(&tracked))
        .expect("应读取未暂存差异")
        .pop()
        .expect("应返回未暂存文件")
        .1;

    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].range, 1..2);
    assert_eq!(unstaged.len(), 1);
    assert_eq!(unstaged[0].range, 0..1);
}

#[test]
fn hunk_operations_stage_restore_and_unstage_independently() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);
    let tracked = PathBuf::from("tracked.txt");
    let original = (0..12)
        .map(|line| format!("line{line}"))
        .collect::<Vec<_>>();
    fs::write(root.join(&tracked), format!("{}\n", original.join("\n"))).expect("应写入基准文件");
    run_in(&root, &["git", "add", "tracked.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "baseline"]);

    let mut changed = original.clone();
    changed[2] = "暂存这一块".into();
    changed[9] = "还原这一块".into();
    fs::write(root.join(&tracked), format!("{}\n", changed.join("\n")))
        .expect("应写入两个独立 hunk");

    let hunks = repository
        .diff_hunks_for_paths(DiffBase::Index, std::slice::from_ref(&tracked))
        .expect("应读取未暂存 hunks")
        .pop()
        .expect("应返回文件")
        .1;
    assert_eq!(hunks.len(), 2);
    repository
        .apply_hunk(GitHunkOperation::Stage, &tracked, &hunks[0])
        .expect("应只暂存第一块");

    let staged = repository
        .diff_hunks_for_paths(DiffBase::Staged, std::slice::from_ref(&tracked))
        .expect("应读取已暂存 hunks")
        .pop()
        .expect("应返回已暂存文件")
        .1;
    let unstaged = repository
        .diff_hunks_for_paths(DiffBase::Index, std::slice::from_ref(&tracked))
        .expect("应读取剩余未暂存 hunks")
        .pop()
        .expect("应返回未暂存文件")
        .1;
    assert_eq!(staged, vec![hunks[0].clone()]);
    assert_eq!(unstaged, vec![hunks[1].clone()]);

    repository
        .apply_hunk(GitHunkOperation::Restore, &tracked, &unstaged[0])
        .expect("应还原第二块");
    assert!(
        repository
            .diff_hunks_for_paths(DiffBase::Index, std::slice::from_ref(&tracked))
            .expect("应读取还原后的差异")
            .into_iter()
            .flat_map(|(_, hunks)| hunks)
            .next()
            .is_none()
    );

    repository
        .apply_hunk(GitHunkOperation::Unstage, &tracked, &staged[0])
        .expect("应取消暂存第一块");
    let unstaged = repository
        .diff_hunks_for_paths(DiffBase::Index, std::slice::from_ref(&tracked))
        .expect("应读取取消暂存后的差异")
        .pop()
        .expect("应返回未暂存文件")
        .1;
    assert_eq!(unstaged, vec![hunks[0].clone()]);
    assert_eq!(
        fs::read_to_string(root.join(&tracked)).expect("应读取工作区文件"),
        format!("{}\n", {
            let mut expected = original;
            expected[2] = "暂存这一块".into();
            expected.join("\n")
        })
    );
}

#[test]
fn staging_a_later_hunk_uses_index_coordinates_instead_of_worktree_line_numbers() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);
    let tracked = PathBuf::from("offset-hunks.txt");
    let original = (0..220)
        .map(|line| format!("line{line}"))
        .collect::<Vec<_>>();
    fs::write(root.join(&tracked), format!("{}\n", original.join("\n"))).expect("应写入基准文件");
    run_in(&root, &["git", "add", "offset-hunks.txt"]);
    run_in(&root, &["git", "commit", "-q", "-m", "baseline"]);

    let mut changed = original.clone();
    changed.splice(
        10..10,
        (0..120).map(|line| format!("前序未暂存新增 {line}")),
    );
    changed.insert(270, "只暂存这一行".into());
    fs::write(root.join(&tracked), format!("{}\n", changed.join("\n")))
        .expect("应写入带大幅行偏移的工作区文本");

    let hunks = repository
        .diff_hunks_for_paths(DiffBase::Index, std::slice::from_ref(&tracked))
        .expect("应读取未暂存 hunks")
        .pop()
        .expect("应返回文件")
        .1;
    assert_eq!(hunks.len(), 2);
    repository
        .apply_hunk(GitHunkOperation::Stage, &tracked, &hunks[1])
        .expect("应暂存后一个 hunk");

    let index_text = repository
        .load_revisions(&[":offset-hunks.txt"])
        .expect("应读取 index 文本")
        .pop()
        .flatten()
        .expect("index 应包含文件");
    let mut expected_index = original.clone();
    expected_index.insert(150, "只暂存这一行".into());
    assert_eq!(
        String::from_utf8(index_text).expect("index 应为 UTF-8"),
        format!("{}\n", expected_index.join("\n"))
    );

    let staged = repository
        .diff_hunks_for_paths(DiffBase::Staged, std::slice::from_ref(&tracked))
        .expect("应读取已暂存 hunk")
        .pop()
        .expect("应返回已暂存文件")
        .1;
    assert_eq!(staged.len(), 1);
    repository
        .apply_hunk(GitHunkOperation::Unstage, &tracked, &staged[0])
        .expect("应取消暂存后一个 hunk");
    let index_text = repository
        .load_revisions(&[":offset-hunks.txt"])
        .expect("应重新读取 index 文本")
        .pop()
        .flatten()
        .expect("index 应包含文件");
    assert_eq!(
        String::from_utf8(index_text).expect("index 应为 UTF-8"),
        format!("{}\n", original.join("\n"))
    );
}

/// 提取 FileStatus 的 index 状态（非 Tracked 视为 Unmodified）。
fn index_status(status: &FileStatus) -> StatusCode {
    match status {
        FileStatus::Tracked { index_status, .. } => *index_status,
        _ => StatusCode::Unmodified,
    }
}

#[test]
fn stage_and_unstage_paths_move_index_state() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);
    let tracked = Path::new("tracked.txt");
    let new_file = Path::new("new.txt");

    // 修改已跟踪文件 + 新建文件。
    fs::write(root.join("tracked.txt"), "修改后的内容\n").expect("应修改文件");
    fs::write(root.join("new.txt"), "新文件\n").expect("应新建文件");

    // 初始：均已未暂存（index 未动）。
    let status = repository.status(&[]).expect("status 应成功");
    let by_path: HashMap<_, _> = status
        .statuses
        .iter()
        .map(|(path, status)| (path.as_path(), status))
        .collect();
    assert_eq!(index_status(by_path[tracked]), StatusCode::Unmodified);
    assert!(by_path[tracked].is_modified());
    assert!(by_path[new_file].is_untracked());

    // 暂存两个路径 → index 出现对应状态（修改 + 新增）。
    repository
        .stage_paths(&[tracked.to_path_buf(), new_file.to_path_buf()])
        .expect("stage 应成功");
    let status = repository.status(&[]).expect("status 应成功");
    let by_path: HashMap<_, _> = status
        .statuses
        .iter()
        .map(|(path, status)| (path.as_path(), status))
        .collect();
    assert_eq!(index_status(by_path[tracked]), StatusCode::Modified);
    assert_eq!(index_status(by_path[new_file]), StatusCode::Added);

    // 取消暂存 → 回到未暂存/未跟踪（新建文件移出 index）。
    repository
        .unstage_paths(&[tracked.to_path_buf(), new_file.to_path_buf()])
        .expect("unstage 应成功");
    let status = repository.status(&[]).expect("status 应成功");
    let by_path: HashMap<_, _> = status
        .statuses
        .iter()
        .map(|(path, status)| (path.as_path(), status))
        .collect();
    assert_eq!(index_status(by_path[tracked]), StatusCode::Unmodified);
    assert!(by_path[new_file].is_untracked());
}

#[test]
fn init_creates_repository_with_configured_branch() {
    let temp_dir = tempfile::tempdir().expect("应创建临时目录");
    let root = temp_dir.path().to_path_buf();

    init(&root, "main").expect("init 应成功");
    assert!(root.join(".git").is_dir(), "应创建 .git 目录");

    // 期望分支由本机全局 init.defaultBranch 决定（未配置则用 fallback），
    // 与实现同路径计算，避免测试依赖具体机器配置。
    let expected = resolve_branch(
        configured_default_branch()
            .expect("读取配置应成功")
            .as_deref(),
        "main",
    );
    let head = run_in(&root, &["git", "symbolic-ref", "--short", "HEAD"]);
    assert_eq!(
        String::from_utf8_lossy(&head.stdout).trim(),
        expected,
        "初始分支应使用 init.defaultBranch 或 fallback"
    );
}

#[test]
fn resolve_branch_uses_configured_or_fallback() {
    assert_eq!(resolve_branch(None, "main"), "main");
    assert_eq!(resolve_branch(Some("dev"), "main"), "dev");
    assert_eq!(resolve_branch(Some("  "), "main"), "main");
}

#[test]
fn commit_creates_commit_and_reports_subject() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);

    fs::write(root.join("tracked.txt"), "修改后的内容\n").expect("应修改文件");
    repository
        .stage_paths(&[PathBuf::from("tracked.txt")])
        .expect("stage 应成功");
    // 标准提交消息：subject + 空行 + body；%s 只取首行。
    repository
        .commit("首行提交\n\n第二行详细说明")
        .expect("commit 应成功");

    // subject 只取首行（%(contents:subject)）。
    let (_, subject) = repository.head_commit().expect("查询应成功");
    assert_eq!(subject.as_deref(), Some("首行提交"));
    // oid 与直接执行 git 对照。
    let oid = run_in(&root, &["git", "rev-parse", "HEAD"]);
    let (queried_oid, _) = repository.head_commit().expect("查询应成功");
    assert_eq!(
        queried_oid.as_deref(),
        Some(String::from_utf8_lossy(&oid.stdout).trim())
    );
}

#[test]
fn head_commit_none_without_head() {
    let temp_dir = tempfile::tempdir().expect("应创建临时目录");
    run_in(temp_dir.path(), &["git", "init", "-q", "-b", "master"]);
    let repository = open_repo(temp_dir.path());

    let (oid, subject) = repository.head_commit().expect("查询应成功");
    assert_eq!(oid, None);
    assert_eq!(subject, None);
}

#[test]
fn commit_fails_without_changes() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);

    // 干净工作树：git 报 nothing to commit，run_command 带 stderr 返回错误。
    assert!(repository.commit("无改动提交").is_err());
}

#[test]
fn uncommit_returns_full_message_and_rewinds_head() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);

    fs::write(root.join("tracked.txt"), "第二次修改\n").expect("应修改文件");
    repository
        .stage_paths(&[PathBuf::from("tracked.txt")])
        .expect("stage 应成功");
    repository
        .commit("第二次提交\n\n详细说明")
        .expect("commit 应成功");

    // uncommit 返回被撤销提交的完整消息（含 body，%B 保留空行），HEAD 回退到 initial。
    let message = repository
        .uncommit()
        .expect("uncommit 应成功")
        .expect("应返回被撤销提交的消息");
    assert_eq!(message, "第二次提交\n\n详细说明");
    let log = run_in(&root, &["git", "log", "-1", "--pretty=format:%s"]);
    assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "initial");

    // --soft：改动保留在 index（staged）。
    let status = repository.status(&[]).expect("status 应成功");
    let by_path: HashMap<_, _> = status
        .statuses
        .iter()
        .map(|(path, status)| (path.as_path(), status))
        .collect();
    assert_eq!(
        index_status(by_path[Path::new("tracked.txt")]),
        StatusCode::Modified
    );
}

#[test]
fn branches_lists_local_branches_with_head_marker() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);
    // for-each-ref 按 refname 字典序输出，断言按名字查找不依赖顺序。
    // 切到新分支后 HEAD 标记应跟随。
    run_in(&root, &["git", "checkout", "-q", "-b", "feature"]);
    let branches = repository.branches().expect("branches 应成功");
    let by_name: HashMap<_, _> = branches
        .iter()
        .map(|branch| (branch.name.as_str(), branch.is_head))
        .collect();
    assert!(!by_name["master"]);
    assert!(by_name["feature"]);

    run_in(&root, &["git", "checkout", "-q", "master"]);
    let branches = repository.branches().expect("branches 应成功");
    assert_eq!(
        branches
            .iter()
            .find(|branch| branch.is_head)
            .expect("应有当前分支"),
        &Branch {
            name: "master".into(),
            is_head: true,
        }
    );
}

#[test]
fn branches_empty_in_empty_repository() {
    let temp_dir = tempfile::tempdir().expect("应创建临时目录");
    run_in(temp_dir.path(), &["git", "init", "-q", "-b", "master"]);
    let repository = open_repo(temp_dir.path());
    assert_eq!(repository.branches().expect("branches 应成功"), vec![]);
}

#[test]
fn branches_has_no_head_marker_when_detached() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);
    run_in(&root, &["git", "checkout", "-q", "--detach"]);
    assert!(
        repository
            .branches()
            .expect("branches 应成功")
            .iter()
            .all(|branch| !branch.is_head)
    );
}

#[test]
fn checkout_switches_branch() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);
    run_in(&root, &["git", "checkout", "-q", "-b", "feature"]);

    repository.checkout("master").expect("checkout 应成功");
    let current = repository
        .branches()
        .expect("branches 应成功")
        .into_iter()
        .find(|branch| branch.is_head)
        .expect("应有当前分支");
    assert_eq!(current.name, "master");
}

#[test]
fn checkout_fails_for_unknown_branch() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);
    assert!(repository.checkout("nope").is_err());
}

#[test]
fn create_branch_creates_and_switches() {
    let (root, _temp) = test_repo();
    let repository = open_repo(&root);

    // base 省略：从当前 HEAD 创建并切换。
    repository
        .create_branch("feature", None)
        .expect("create_branch 应成功");
    let current = repository
        .branches()
        .expect("branches 应成功")
        .into_iter()
        .find(|branch| branch.is_head)
        .expect("应有当前分支");
    assert_eq!(current.name, "feature");

    // 显式 base：从 master 创建并切换。
    repository
        .create_branch("from_master", Some("master"))
        .expect("create_branch 应成功");
    let current = repository
        .branches()
        .expect("branches 应成功")
        .into_iter()
        .find(|branch| branch.is_head)
        .expect("应有当前分支");
    assert_eq!(current.name, "from_master");
}
