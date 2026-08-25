//! Worktree —— 项目目录快照层。
//!
//! 对齐 Zed worktree crate 的职责边界：目录遍历、扫描排除规则、git 仓库发现与路径命名语义住在这一层；
//! git 状态由 `Project` 从 `GitStore` 查询后填充到条目。
//! 本层只提供静态目录查询（`children`），展开、深度与可见行是项目树视图状态，由 UI 层自行构建。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use zcv_git::{FileStatus, GitRepository, RealGitRepository};

/// `.git` 目录名（仓库发现用）。
const DOT_GIT: &str = ".git";

/// 目录快照层的静态条目：由 Worktree 遍历产出，不含展开/深度等视图状态。
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// git 状态（目录聚合/文件精确由 Project 查询填充；Worktree 产出时恒 None）。
    pub git_status: Option<FileStatus>,
}

/// 项目目录快照层：持有根路径与扫描排除规则，提供静态目录查询。
pub(crate) struct Worktree {
    root: PathBuf,
    filter: TreeFilter,
}

#[derive(Clone)]
pub(crate) struct WorktreeSearchPlan {
    pub(crate) root: PathBuf,
    filter: TreeFilter,
}

impl Worktree {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            filter: TreeFilter::new(&[]),
        }
    }

    /// 更换项目根目录（展开与选中状态由 UI 层重置，本层只换根）。
    pub(crate) fn set_root(&mut self, root: PathBuf) {
        self.root = root;
    }

    /// 更新扫描排除规则（设置变化时由 Project 调用）。
    pub(crate) fn set_exclusions(&mut self, exclusions: &[String]) {
        self.filter = TreeFilter::new(exclusions);
    }

    /// 读取 `dir` 的直接子项：目录优先、名称升序，扫描排除名单命中即过滤。
    ///
    /// 只返回静态条目；git 状态由 Project 查询后填充，展开与深度由 UI 层决定。
    pub(crate) fn children(&self, dir: &Path) -> Vec<WorktreeEntry> {
        let mut entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
            Err(_) => return Vec::new(),
        };
        entries.sort_by(|a, b| {
            let a_dir = a.is_dir();
            let b_dir = b.is_dir();
            if a_dir != b_dir {
                b_dir.cmp(&a_dir)
            } else {
                a.file_name().cmp(&b.file_name())
            }
        });
        entries
            .into_iter()
            .filter_map(|entry| {
                let name = entry.file_name()?.to_string_lossy().to_string();
                // 扫描排除名单命中的条目根本不加载。
                let rel = entry.strip_prefix(&self.root).ok()?;
                if self.filter.is_excluded(rel) {
                    return None;
                }
                let is_dir = entry.is_dir();
                Some(WorktreeEntry {
                    path: entry,
                    name,
                    is_dir,
                    git_status: None,
                })
            })
            .collect()
    }

    pub(crate) fn search_plan(&self) -> WorktreeSearchPlan {
        WorktreeSearchPlan {
            root: self.root.clone(),
            filter: self.filter.clone(),
        }
    }
}

/// 项目树的过滤规则：扫描排除（glob 名单）。
///
/// file_scan_exclusions 命中的条目根本不在行模型中加载；
/// 忽略（gitignore/info/exclude）由 git 状态统一判定（`FileStatus::Ignored`）。
#[derive(Clone)]
struct TreeFilter {
    /// 用户配置的扫描排除 glob。
    exclusions: GlobSet,
}

impl WorktreeSearchPlan {
    pub(crate) fn is_excluded(&self, path: &Path) -> bool {
        path.strip_prefix(&self.root)
            .is_ok_and(|relative| self.filter.is_excluded(relative))
    }
}

impl TreeFilter {
    fn new(exclusions: &[String]) -> Self {
        let mut builder = GlobSetBuilder::new();
        for glob in exclusions {
            if let Ok(glob) = Glob::new(glob) {
                builder.add(glob);
            }
        }
        Self {
            exclusions: builder.build().unwrap_or_default(),
        }
    }

    /// 路径的任一祖先命中排除名单即排除。
    fn is_excluded(&self, rel_path: &Path) -> bool {
        rel_path
            .ancestors()
            .any(|ancestor| self.exclusions.is_match(ancestor))
    }
}

// ── git 仓库发现 ─────────────────────────────────────────────────────
// 仓库发现是项目扫描的目录遍历决策，git 层只负责打开已知 `.git` 目录与命令执行。

/// 在 `path` 自身或任一祖先目录中向上查找 `.git` 目录，命中则打开仓库。
///
/// 只认 `.git` 目录：worktree/子模块的 `.git` 是文件（`gitdir:` 指针），v1 不支持这类布局，向上继续查找外层普通仓库。
pub(crate) fn discover_git_repository(path: &Path) -> anyhow::Result<Option<RealGitRepository>> {
    for dir in path.ancestors() {
        let dot_git = dir.join(DOT_GIT);
        if dot_git.is_dir() {
            return RealGitRepository::open(&dot_git).map(Some);
        }
    }
    Ok(None)
}

/// 在 `root` 下遍历寻找所有 `.git` 目录，生成嵌套仓库列表。
///
/// 找到仓库后跳过其 `.git` 子树（objects/refs 等）不深入；
/// 跳过常见重型依赖目录，避免 node_modules、target 这类目录拖慢遍历。
pub(crate) fn find_git_repositories(root: &Path) -> anyhow::Result<Vec<RealGitRepository>> {
    fn visit(dir: &Path, repositories: &mut Vec<RealGitRepository>) -> anyhow::Result<()> {
        let dot_git = dir.join(DOT_GIT);
        if dot_git.is_dir() {
            repositories.push(RealGitRepository::open(&dot_git)?);
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            // 找到仓库后不再深入其 .git 子树（objects/refs 等）。
            if path.is_dir()
                && entry.file_name() != DOT_GIT
                && !is_heavy_dependency_dir(entry.file_name().as_encoded_bytes())
            {
                visit(&path, repositories)?;
            }
        }
        Ok(())
    }

    let mut repositories = Vec::new();
    visit(root, &mut repositories)?;
    Ok(repositories)
}

/// 合并发现 root 相关的全部仓库：root 下的所有嵌套仓库（含 root 自身）+ root 所在的外层仓库（若有）。
///
/// 返回顺序：外层仓库（若存在且未在嵌套集合中）在最前，其余按 `find_git_repositories` 的 DFS 顺序。
///
/// 外层仓库必须前置：root 在外层仓库内时 find 只返回嵌套仓库，不补上祖先会导致 root 直下文件匹配不到任何仓库（状态/hunks 全部丢失）。
/// 去重依据 working_directory：两条发现路径都经 `RealGitRepository::open` 的 canonicalize，比较天然一致。
pub(crate) fn discover_repositories(root: &Path) -> anyhow::Result<Vec<RealGitRepository>> {
    let mut repositories = find_git_repositories(root)?;
    let known: HashSet<&Path> = repositories
        .iter()
        .map(|repository| repository.working_directory())
        .collect();
    if let Some(ancestor) = discover_git_repository(root)?
        && !known.contains(ancestor.working_directory())
    {
        repositories.insert(0, ancestor);
    }
    Ok(repositories)
}

/// 常见重型依赖目录，其内部的 `.git` 不视为独立仓库。
fn is_heavy_dependency_dir(name: &[u8]) -> bool {
    matches!(
        name,
        b"node_modules" | b"target" | b"dist" | b"build" | b".venv" | b"venv" | b"__pycache__"
    )
}

// ── 路径命名语义 ────────────────────────────────────────────────────

/// 重命名目标：条目必须与原名在同一父目录内（只改名称，不允许改路径）。
pub fn rename_destination(from: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let parent = from
        .parent()
        .ok_or_else(|| anyhow::anyhow!("条目没有父目录"))?;
    entry_destination(parent, name)
}

fn entry_destination(parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(!name.is_empty(), "名称不能为空");
    anyhow::ensure!(name != "." && name != "..", "名称不能是 {name}");
    anyhow::ensure!(
        !name.contains(['/', '\\', '\0']),
        "名称不能包含路径分隔符或空字符"
    );
    Ok(parent.join(name))
}

/// 新建条目目标：`/` 结尾表示目录，支持 `src/components/button.rs` 这类嵌套相对路径。
#[derive(Debug, PartialEq, Eq)]
pub struct NewEntryDestination {
    pub path: PathBuf,
    pub is_dir: bool,
}

pub fn new_entry_destination(parent: &Path, input: &str) -> anyhow::Result<NewEntryDestination> {
    anyhow::ensure!(!input.trim().is_empty(), "名称不能为空");
    anyhow::ensure!(!input.starts_with('/'), "新条目必须使用相对路径");
    anyhow::ensure!(!input.contains(['\\', '\0']), "名称不能包含反斜杠或空字符");

    let is_dir = input.ends_with('/');
    let relative = input.trim_end_matches('/');
    anyhow::ensure!(!relative.is_empty(), "名称不能为空");
    let mut path = parent.to_path_buf();
    for component in relative.split('/') {
        anyhow::ensure!(!component.trim().is_empty(), "路径不能包含空名称");
        anyhow::ensure!(
            component != "." && component != "..",
            "路径不能包含 {component}"
        );
        path.push(component);
    }

    Ok(NewEntryDestination { path, is_dir })
}

/// 把路径按 `from → to` 的重命名迁移（条目自身与祖先路径都换新前缀）。
pub fn translate_path(path: &Path, from: &Path, to: &Path) -> PathBuf {
    match path.strip_prefix(from) {
        // 条目自身重命名时后缀为空：直接取 to。
        // `to.join(空路径)` 会追加尾随斜杠，保存这类路径会触发 Not a directory。
        Ok(suffix) if suffix.as_os_str().is_empty() => to.to_path_buf(),
        Ok(suffix) => to.join(suffix),
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use zcv_git::GitRepository;

    use super::*;
    use crate::test_support::{run_git, test_git_repo};

    #[test]
    fn children_return_sorted_static_entries_without_git_status() {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        std::fs::create_dir_all(directory.path().join("zebra_dir")).expect("应创建目录");
        std::fs::write(directory.path().join("apple.rs"), "fn main() {}").expect("应创建文件");
        std::fs::write(directory.path().join("banana.rs"), "fn main() {}").expect("应创建文件");

        let worktree = Worktree::new(directory.path().to_path_buf());
        let entries = worktree.children(directory.path());

        // 目录优先、名称升序；git 状态由 Project 注入，本层恒 None。
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["zebra_dir", "apple.rs", "banana.rs"]
        );
        assert!(entries.iter().all(|entry| entry.git_status.is_none()));
        // 不存在或不可读目录返回空。
        assert!(
            worktree
                .children(&directory.path().join("missing"))
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

        let mut worktree = Worktree::new(directory.path().to_path_buf());
        worktree.set_exclusions(&["**/target".to_string()]);

        assert!(
            !worktree
                .children(directory.path())
                .iter()
                .any(|entry| entry.path == target),
            "排除名单命中的目录不应出现"
        );
        assert!(
            worktree.children(&target).is_empty(),
            "被排除目录内部也不加载"
        );
        assert!(
            worktree
                .children(directory.path())
                .iter()
                .any(|entry| entry.path == visible)
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
        assert_eq!(
            repo.working_directory(),
            root.canonicalize().expect("应可 canonicalize")
        );
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
        assert_eq!(
            repo.working_directory(),
            root.canonicalize().expect("应可 canonicalize")
        );
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
        assert!(work_dirs.contains(&root.canonicalize().expect("应可 canonicalize").as_path()));
        assert!(
            work_dirs.contains(
                &root
                    .join("nested")
                    .canonicalize()
                    .expect("应可 canonicalize")
                    .as_path()
            )
        );
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
            root.canonicalize().expect("应可 canonicalize").as_path()
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
            outer.canonicalize().expect("应可 canonicalize").as_path()
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
            root.canonicalize().expect("应可 canonicalize").as_path()
        );
    }

    #[test]
    fn discover_repositories_none_outside_any_repo() {
        let directory = tempfile::tempdir().expect("应创建临时目录");
        let repos = discover_repositories(directory.path()).expect("discover 应成功");
        assert!(repos.is_empty());
    }
}
