//! 测试共享基建：临时 git 仓库的创建与命令执行。
//!
//! project / worktree / git_store 的测试各自持有同构实现，收敛于此避免三份克隆。

use std::path::{Path, PathBuf};

/// 创建带一个初始提交的临时 git 仓库，返回 (仓库根, 目录句柄)。
pub(crate) fn test_git_repo() -> (PathBuf, tempfile::TempDir) {
    let temp_dir = tempfile::tempdir().expect("应创建临时目录");
    let root = temp_dir.path().to_path_buf();
    run_git(&root, &["init", "-q", "-b", "master"]);
    run_git(&root, &["config", "user.email", "test@example.com"]);
    run_git(&root, &["config", "user.name", "Test User"]);
    std::fs::write(root.join("tracked.txt"), "第一行\n第二行\n").expect("应写入初始文件");
    run_git(&root, &["add", "tracked.txt"]);
    run_git(&root, &["commit", "-q", "-m", "initial"]);
    (root, temp_dir)
}

/// 在指定目录执行 git 命令（测试断言成功）。
pub(crate) fn run_git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("应执行成功");
    assert!(
        output.status.success(),
        "git {:?} 失败：{}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// 读取指定目录仓库的 `master` 分支 commit（测试断言用）。
pub(crate) fn rev_parse(dir: &Path) -> String {
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
}
