use zcv_git::GitRepository;
use zcv_path::AbsolutePathBuf;

use super::*;
use crate::test_support::{run_git, test_git_repo};

fn absolute(path: &Path) -> AbsolutePathBuf {
    AbsolutePathBuf::new(path.to_path_buf()).expect("测试工作区路径应为绝对路径")
}

fn canonical_path(path: &Path) -> PathBuf {
    AbsolutePathBuf::canonicalize(path)
        .expect("测试路径应可规范化")
        .into_path_buf()
}

#[test]
fn children_return_sorted_static_entries() {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    std::fs::create_dir_all(directory.path().join("zebra_dir")).expect("应创建目录");
    std::fs::write(directory.path().join("apple.rs"), "fn main() {}").expect("应创建文件");
    std::fs::write(directory.path().join("banana.rs"), "fn main() {}").expect("应创建文件");

    let worktree = Worktree::new(absolute(directory.path()));
    let entries = children_sorted(directory.path(), directory.path(), &worktree.filter());

    // 目录优先、名称升序；Git 状态不属于目录快照层。
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["zebra_dir", "apple.rs", "banana.rs"]
    );
    // 不存在或不可读目录返回空。
    assert!(
        children_sorted(
            &directory.path().join("missing"),
            directory.path(),
            &worktree.filter()
        )
        .is_empty()
    );
}

#[test]
fn file_scan_exclusions_hide_entries_and_their_children() {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let target = directory.path().join("target");
    std::fs::create_dir_all(target.join("debug")).expect("应创建排除目录");
    std::fs::write(target.join("debug").join("app"), "binary").expect("应创建排除文件");
    let visible = directory.path().join("main.rs");
    std::fs::write(&visible, "fn main() {}").expect("应创建可见文件");

    let mut worktree = Worktree::new(absolute(directory.path()));
    worktree.set_exclusions(&["**/target".to_string()]);

    assert!(
        !children_sorted(directory.path(), directory.path(), &worktree.filter())
            .iter()
            .any(|entry| entry.path.as_path() == target),
        "排除名单命中的目录不应出现"
    );
    assert!(
        children_sorted(&target, directory.path(), &worktree.filter()).is_empty(),
        "被排除目录内部也不加载"
    );
    assert!(
        children_sorted(directory.path(), directory.path(), &worktree.filter())
            .iter()
            .any(|entry| entry.path.as_path() == visible)
    );
}

#[test]
fn collect_visible_entries_returns_root_then_expanded_descendants() {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let src = directory.path().join("src");
    std::fs::create_dir_all(src.join("feature")).expect("应创建嵌套目录");
    std::fs::write(src.join("feature").join("mod.rs"), "x").expect("应创建文件");
    std::fs::write(directory.path().join("root.rs"), "x").expect("应创建文件");
    let mut worktree = Worktree::new(absolute(directory.path()));
    worktree.set_exclusions(&["**/root.rs".to_string()]);

    // 只展开根：行集 = [根, src]（root.rs 被排除，src 折叠不深入）。
    let root = absolute(directory.path());
    let expanded = HashSet::from([root.clone()]);
    let rows = collect_visible_entries(&root, &expanded, &worktree.filter());
    assert_eq!(rows.len(), 2);
    assert!(rows[0].is_dir);
    assert_eq!(rows[0].path.as_path(), directory.path());
    assert_eq!(rows[1].path.as_path(), src);

    // 根、src、feature 都展开：行集 = [根, src, feature, mod.rs]，排序与 children 一致。
    let expanded = HashSet::from([root.clone(), absolute(&src), absolute(&src.join("feature"))]);
    let rows = collect_visible_entries(&root, &expanded, &worktree.filter());
    let paths: Vec<_> = rows.iter().map(|row| row.path.clone()).collect();
    assert_eq!(
        paths,
        vec![
            absolute(directory.path()),
            absolute(&src),
            absolute(&src.join("feature")),
            absolute(&src.join("feature").join("mod.rs")),
        ]
    );
}

#[test]
fn rename_destination_accepts_one_name_and_rejects_paths() {
    let source = Path::new("/project/src/main.rs");

    assert_eq!(
        rename_destination(source, "lib.rs").unwrap(),
        Path::new("/project/src/lib.rs")
    );
    for invalid in ["", ".", "..", "nested/lib.rs", "nested\\lib.rs"] {
        assert!(rename_destination(source, invalid).is_err());
    }
}

#[test]
fn translate_path_migrates_entry_and_ancestors_without_trailing_slash() {
    // 条目自身重命名：后缀为空，结果必须等于 to（不得带尾随斜杠）。
    assert_eq!(
        translate_path(
            Path::new("/project/src/main.rs"),
            Path::new("/project/src/main.rs"),
            Path::new("/project/src/lib.rs")
        ),
        PathBuf::from("/project/src/lib.rs")
    );
    // 目录重命名：其下条目跟随迁移。
    assert_eq!(
        translate_path(
            Path::new("/project/src/main.rs"),
            Path::new("/project/src"),
            Path::new("/project/lib")
        ),
        PathBuf::from("/project/lib/main.rs")
    );
    // 不匹配路径保持原样。
    assert_eq!(
        translate_path(
            Path::new("/project/other.rs"),
            Path::new("/project/src"),
            Path::new("/project/lib")
        ),
        PathBuf::from("/project/other.rs")
    );
}

#[test]
fn new_entry_destination_uses_a_trailing_slash_for_nested_directories() {
    let parent = Path::new("/project");

    assert_eq!(
        new_entry_destination(parent, "src/components/button.rs").unwrap(),
        NewEntryDestination {
            path: PathBuf::from("/project/src/components/button.rs"),
            is_dir: false,
        }
    );
    assert_eq!(
        new_entry_destination(parent, "assets/icons/").unwrap(),
        NewEntryDestination {
            path: PathBuf::from("/project/assets/icons"),
            is_dir: true,
        }
    );
    for invalid in [
        "",
        "/absolute",
        "src//main.rs",
        "../outside",
        "src\\main.rs",
    ] {
        assert!(new_entry_destination(parent, invalid).is_err());
    }
}

#[test]
fn discovers_repository_from_any_ancestor() {
    let (root, _temp) = test_git_repo();
    let nested = root.join("src/deep/nested");
    std::fs::create_dir_all(&nested).expect("应创建嵌套目录");

    let repo = discover_git_repository(&nested)
        .expect("discover 应成功")
        .expect("应发现外层仓库");
    // open() 会 canonicalize，macOS 上 /var 是 /private/var 的符号链接。
    assert_eq!(repo.working_directory(), canonical_path(&root));
}

#[test]
fn discover_returns_none_outside_repository() {
    let temp_dir = tempfile::tempdir().expect("应创建临时目录");
    assert!(
        discover_git_repository(temp_dir.path())
            .expect("discover 应成功")
            .is_none()
    );
}

#[test]
fn discover_skips_submodule_git_file() {
    let (root, _temp) = test_git_repo();
    let submodule = root.join("submodule");
    std::fs::create_dir_all(&submodule).expect("应创建子模块目录");
    std::fs::write(
        submodule.join(".git"),
        "gitdir: ../.git/modules/submodule\n",
    )
    .expect("应写入 .git 文件");

    let repo = discover_git_repository(&submodule)
        .expect("discover 应成功")
        .expect("应向上找到外层仓库");
    assert_eq!(repo.working_directory(), canonical_path(&root));
}

#[test]
fn find_git_repositories_finds_nested_repos_and_skips_heavy_dirs() {
    let (root, _temp) = test_git_repo();
    std::fs::create_dir_all(root.join("nested")).expect("应创建嵌套目录");
    run_git(&root.join("nested"), &["init", "-q"]);
    std::fs::create_dir_all(root.join("node_modules/pkg")).expect("应创建依赖目录");
    run_git(&root.join("node_modules/pkg"), &["init", "-q"]);

    let repos = find_git_repositories(&root).expect("find 应成功");
    // 根仓库 + 嵌套仓库；node_modules 内的仓库被排除。
    assert_eq!(repos.len(), 2);
    let work_dirs: Vec<_> = repos.iter().map(|repo| repo.working_directory()).collect();
    // open() 会 canonicalize（macOS 上 /var 是 /private/var 的符号链接）。
    let expected_root = canonical_path(&root);
    assert!(work_dirs.contains(&expected_root.as_path()));
    let expected_nested = canonical_path(&root.join("nested"));
    assert!(work_dirs.contains(&expected_nested.as_path()));
}

#[test]
fn discover_repositories_finds_root_and_nested() {
    let (root, _temp) = test_git_repo();
    std::fs::create_dir_all(root.join("nested")).expect("应创建嵌套目录");
    run_git(&root.join("nested"), &["init", "-q"]);
    std::fs::create_dir_all(root.join("node_modules/pkg")).expect("应创建依赖目录");
    run_git(&root.join("node_modules/pkg"), &["init", "-q"]);

    let repos = discover_repositories(&root).expect("discover 应成功");
    // 根仓库 + 嵌套仓库；node_modules 内的仓库被排除；root 仓库不重复。
    assert_eq!(repos.len(), 2);
    assert_eq!(
        repos[0].working_directory(),
        canonical_path(&root).as_path()
    );
}

#[test]
fn discover_repositories_prepends_ancestor() {
    // root 不是仓库，但位于外层仓库内，且自身包含嵌套仓库。
    let (outer, _temp) = test_git_repo();
    let root = outer.join("proj");
    std::fs::create_dir_all(&root).expect("应创建项目目录");
    std::fs::create_dir_all(root.join("nested")).expect("应创建嵌套目录");
    run_git(&root.join("nested"), &["init", "-q"]);

    let repos = discover_repositories(&root).expect("discover 应成功");
    // 外层仓库（祖先前置）+ 嵌套仓库。
    assert_eq!(repos.len(), 2);
    assert_eq!(
        repos[0].working_directory(),
        canonical_path(&outer).as_path()
    );
}

#[test]
fn discover_repositories_dedups_root() {
    let (root, _temp) = test_git_repo();
    let repos = discover_repositories(&root).expect("discover 应成功");
    // discover 与 find 命中同一仓库，去重后不重复。
    assert_eq!(repos.len(), 1);
    assert_eq!(
        repos[0].working_directory(),
        canonical_path(&root).as_path()
    );
}

#[test]
fn discover_repositories_none_outside_any_repo() {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let repos = discover_repositories(directory.path()).expect("discover 应成功");
    assert!(repos.is_empty());
}
