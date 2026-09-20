use super::*;
use FileStatus::*;

fn parse(output: &str) -> Vec<(PathBuf, FileStatus)> {
    GitStatus::from_bytes(output.as_bytes())
        .expect("解析应成功")
        .statuses
}

#[test]
fn has_staged_and_has_unstaged_split_index_and_worktree() {
    // 未暂存修改：index 干净、worktree 有差异。
    let unstaged = Tracked {
        index_status: StatusCode::Unmodified,
        worktree_status: StatusCode::Modified,
    };
    assert!(!unstaged.has_staged());
    assert!(unstaged.has_unstaged());

    // 已暂存修改：index 有差异、worktree 干净。
    let staged = Tracked {
        index_status: StatusCode::Modified,
        worktree_status: StatusCode::Unmodified,
    };
    assert!(staged.has_staged());
    assert!(!staged.has_unstaged());

    // 部分暂存：两侧都有。
    let partial = Tracked {
        index_status: StatusCode::Added,
        worktree_status: StatusCode::Deleted,
    };
    assert!(partial.has_staged());
    assert!(partial.has_unstaged());

    // 未跟踪归入未暂存；忽略与冲突不参与暂存。
    assert!(Untracked.has_unstaged());
    assert!(!Untracked.has_staged());
    assert!(!Ignored.has_staged());
    assert!(!Ignored.has_unstaged());
    assert!(!Unmerged.has_staged());
    assert!(!Unmerged.has_unstaged());
}

#[test]
fn parses_all_status_code_combinations() {
    let output = [
        " M src/main.rs",
        "M  staged.rs",
        "MM both.rs",
        "A  added.txt",
        " D deleted.txt",
        "D  index_deleted.txt",
        "?? untracked.txt",
        "!! ignored.log",
        "UU conflicted.txt",
    ]
    .join("\0");

    // from_bytes 按路径排序，断言用路径索引而非输入顺序。
    let statuses = parse(&output);
    assert_eq!(statuses.len(), 9);
    let by_path: HashMap<&str, FileStatus> = statuses
        .iter()
        .map(|(path, status)| (path.to_str().expect("路径应可转 str"), *status))
        .collect();
    assert!(matches!(
        by_path["src/main.rs"],
        Tracked {
            index_status: StatusCode::Unmodified,
            worktree_status: StatusCode::Modified
        }
    ));
    assert!(matches!(
        by_path["staged.rs"],
        Tracked {
            index_status: StatusCode::Modified,
            worktree_status: StatusCode::Unmodified
        }
    ));
    assert!(matches!(
        by_path["both.rs"],
        Tracked {
            index_status: StatusCode::Modified,
            worktree_status: StatusCode::Modified
        }
    ));
    assert!(by_path["added.txt"].is_created());
    assert!(by_path["deleted.txt"].is_deleted());
    assert!(by_path["index_deleted.txt"].is_deleted());
    assert!(by_path["untracked.txt"].is_untracked());
    assert!(by_path["ignored.log"].is_ignored());
}

#[test]
fn skips_untracked_directories() {
    let output = ["?? new-dir/", "?? dir/file.txt"].join("\0");
    let statuses = parse(&output);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].0, PathBuf::from("dir/file.txt"));
}

#[test]
fn keeps_ignored_directories_without_trailing_slash() {
    // --ignored=matching 下被忽略目录输出为 `!! dir/`，需要保留
    // （目录不展开的依据），路径去掉尾部斜杠。
    let output = ["!! node_modules/", "!! ignored.log", "?? src/new.rs"].join("\0");
    let statuses = parse(&output);
    assert_eq!(statuses.len(), 3);
    assert_eq!(statuses[0].0, PathBuf::from("ignored.log"));
    assert!(statuses[0].1.is_ignored());
    assert_eq!(statuses[1].0, PathBuf::from("node_modules"));
    assert!(statuses[1].1.is_ignored());
    assert_eq!(statuses[2].0, PathBuf::from("src/new.rs"));
    assert!(statuses[2].1.is_untracked());
}

#[test]
fn preserves_paths_with_spaces_and_unicode() {
    let output = "?? 带 空格 的文件.txt\0".to_owned();
    let statuses = parse(&output);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].0, PathBuf::from("带 空格 的文件.txt"));
}

#[test]
fn rejects_invalid_status_code() {
    let output = "Z  invalid.txt";
    assert!(GitStatus::from_bytes(output.as_bytes()).is_err());
}

#[test]
fn sorts_entries_by_path() {
    let output = ["?? b.txt", "?? a.txt", "?? c.txt"].join("\0");
    let statuses = parse(&output);
    let paths: Vec<_> = statuses.iter().map(|(path, _)| path).collect();
    assert_eq!(
        paths,
        vec![
            &PathBuf::from("a.txt"),
            &PathBuf::from("b.txt"),
            &PathBuf::from("c.txt")
        ]
    );
}

#[test]
fn parses_numstat() {
    let output = "5\t2\tsrc/main.rs\0-\t-\timage.png\0";
    let entries = parse_numstat(output.as_bytes());
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries.get(&PathBuf::from("src/main.rs")),
        Some(&DiffStat {
            added: 5,
            deleted: 2
        })
    );
    // 二进制文件计数为 0。
    assert_eq!(
        entries.get(&PathBuf::from("image.png")),
        Some(&DiffStat {
            added: 0,
            deleted: 0
        })
    );
}

#[test]
fn parses_numstat_with_unicode_paths() {
    let output = "1\t0\t中文 路径.rs\0";
    let entries = parse_numstat(output.as_bytes());
    assert_eq!(
        entries.get(&PathBuf::from("中文 路径.rs")),
        Some(&DiffStat {
            added: 1,
            deleted: 0
        })
    );
}

#[test]
fn parses_branch_header_with_upstream_and_counts() {
    let output = "## master...origin/master [ahead 1, behind 2]\0";
    let status = GitStatus::from_bytes(output.as_bytes()).expect("应解析成功");
    let branch = status.branch.expect("应有分支头行");
    assert_eq!(branch.branch.as_deref(), Some("master"));
    assert_eq!(branch.upstream.as_deref(), Some("origin/master"));
    assert_eq!(branch.ahead, 1);
    assert_eq!(branch.behind, 2);
    assert!(status.statuses.is_empty(), "头行不应进入状态表");
}

#[test]
fn parses_branch_header_without_upstream() {
    for header in [
        "## main",
        "## HEAD (no branch)",
        "## No commits yet on main",
    ] {
        let status = GitStatus::from_bytes(format!("{header}\0").as_bytes()).expect("应解析成功");
        let branch = status.branch.expect("应有分支头行");
        assert_eq!(branch.upstream, None, "{header} 不应有 upstream");
        assert_eq!((branch.ahead, branch.behind), (0, 0));
        // 普通分支名可识别；detached 与空仓库无分支名。
        let expect_branch = header == "## main";
        assert_eq!(branch.branch.is_some(), expect_branch, "{header}");
        if let Some(name) = &branch.branch {
            assert_eq!(name, "main");
        }
    }
}

#[test]
fn parses_gone_upstream_as_zero_counts() {
    let output = "## main...origin/main [gone]\0";
    let status = GitStatus::from_bytes(output.as_bytes()).expect("应解析成功");
    let branch = status.branch.expect("应有分支头行");
    assert_eq!(branch.branch.as_deref(), Some("main"));
    assert_eq!(branch.upstream.as_deref(), Some("origin/main"));
    assert_eq!((branch.ahead, branch.behind), (0, 0));
}

#[test]
fn unparseable_branch_header_does_not_fail_parsing() {
    for output in ["## \0", "## main...origin/main [ahead x]\0"] {
        let status = GitStatus::from_bytes(output.as_bytes()).expect("应解析成功");
        assert!(
            status.statuses.is_empty(),
            "病理头行不应产生状态条目：{output:?}"
        );
    }
}

#[test]
fn branch_header_mixed_with_file_entries() {
    let output = "## main...origin/main [ahead 1]\0?? a.txt\0 M b.txt\0";
    let status = GitStatus::from_bytes(output.as_bytes()).expect("应解析成功");
    let branch = status.branch.expect("应有分支头行");
    assert_eq!((branch.ahead, branch.behind), (1, 0));
    let paths: Vec<_> = status.statuses.iter().map(|(path, _)| path).collect();
    assert_eq!(paths, [&PathBuf::from("a.txt"), &PathBuf::from("b.txt")]);
}
