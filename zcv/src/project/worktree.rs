//! Worktree —— 项目目录快照层。
//!
//! 对齐 Zed worktree crate 的职责边界：目录遍历、扫描排除规则、git 仓库发现与路径命名语义住在这一层；git 状态由 `Project` 从 `GitStore` 查询后合并进行模型。
//! UI 组件只消费本层产出的行模型，不直接触碰文件系统。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use zcv_git::{FileStatus, RealGitRepository};

/// `.git` 目录名（仓库发现用）。
const DOT_GIT: &str = ".git";

/// 项目树的一行（领域行模型）：由 Worktree 遍历产出。
#[derive(Debug, Clone)]
pub(crate) struct TreeRow {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) depth: usize,
    pub(crate) is_dir: bool,
    pub(crate) expanded: bool,
    /// git 状态（决定文件名颜色与忽略淡显；None 表示无状态）。
    pub(crate) git_status: Option<FileStatus>,
}

/// 项目目录快照层：持有根路径与扫描排除规则，按展开状态产出可见行。
pub(crate) struct Worktree {
    root: PathBuf,
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

    /// 生成可见行：根行 + 按展开状态递归收集子项。
    ///
    /// `git_status` 在遍历时查询（由 Project 注入 GitStore 查询）：
    /// 命中忽略的目录不展开内容，避免 node_modules 这类目录撑爆行模型。
    pub(crate) fn visible_entries(
        &self,
        expanded: &HashSet<PathBuf>,
        git_status: impl Fn(&Path, bool) -> Option<FileStatus>,
    ) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        let root_name = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.root.to_string_lossy().to_string());
        let root_expanded = expanded.contains(&self.root);
        rows.push(TreeRow {
            path: self.root.clone(),
            name: root_name,
            depth: 0,
            is_dir: true,
            expanded: root_expanded,
            git_status: None,
        });
        if root_expanded {
            self.collect_children(&self.root, 1, expanded, &git_status, &mut rows);
        }
        rows
    }

    /// 递归收集目录子项；忽略（gitignored）目录不展开内容。
    fn collect_children(
        &self,
        dir: &Path,
        depth: usize,
        expanded: &HashSet<PathBuf>,
        git_status: &impl Fn(&Path, bool) -> Option<FileStatus>,
        rows: &mut Vec<TreeRow>,
    ) {
        let mut entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
            Err(_) => return,
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
        for entry in entries {
            let name = match entry.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };
            let is_dir = entry.is_dir();
            // 扫描排除名单命中的条目根本不加载。
            let Ok(rel) = entry.strip_prefix(&self.root) else {
                continue;
            };
            if self.filter.is_excluded(rel) {
                continue;
            }
            let status = git_status(&entry, is_dir);
            let is_expanded = expanded.contains(&entry);
            rows.push(TreeRow {
                path: entry.clone(),
                name,
                depth,
                is_dir,
                expanded: is_expanded,
                git_status: status,
            });
            // 被忽略的目录不展开内容，避免 node_modules 这类目录撑爆行模型。
            if is_dir && is_expanded && !matches!(status, Some(FileStatus::Ignored)) {
                self.collect_children(&entry, depth + 1, expanded, git_status, rows);
            }
        }
    }
}

/// 项目树的过滤规则：扫描排除（glob 名单）。
///
/// file_scan_exclusions 命中的条目根本不在行模型中加载；
/// 忽略（gitignore/info/exclude）由 git 状态统一判定（`FileStatus::Ignored`）。
struct TreeFilter {
    /// 用户配置的扫描排除 glob。
    exclusions: GlobSet,
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
// 仓库发现是项目扫描的目录遍历决策，zcv-git 只负责打开已知 `.git` 目录与命令执行。

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

/// 常见重型依赖目录，其内部的 `.git` 不视为独立仓库。
fn is_heavy_dependency_dir(name: &[u8]) -> bool {
    matches!(
        name,
        b"node_modules" | b"target" | b"dist" | b"build" | b".venv" | b"venv" | b"__pycache__"
    )
}

// ── 路径命名语义 ────────────────────────────────────────────────────

/// 重命名目标：条目必须与原名在同一父目录内（只改名称，不允许改路径）。
pub(crate) fn rename_destination(from: &Path, name: &str) -> anyhow::Result<PathBuf> {
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
pub(crate) struct NewEntryDestination {
    pub(crate) path: PathBuf,
    pub(crate) is_dir: bool,
}

pub(crate) fn new_entry_destination(
    parent: &Path,
    input: &str,
) -> anyhow::Result<NewEntryDestination> {
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
pub(crate) fn translate_path(path: &Path, from: &Path, to: &Path) -> PathBuf {
    path.strip_prefix(from)
        .map_or_else(|_| path.to_path_buf(), |suffix| to.join(suffix))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use zcv_git::GitRepository;

    use super::*;

    /// 构造按注入表查询的 git 状态闭包（测试替身）。
    fn git_status_from(
        statuses: &HashMap<PathBuf, FileStatus>,
    ) -> impl Fn(&Path, bool) -> Option<FileStatus> + '_ {
        |path, _| statuses.get(path).cloned()
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
        let expanded = HashSet::from([directory.path().to_path_buf(), target.clone()]);
        let rows = worktree.visible_entries(&expanded, git_status_from(&HashMap::new()));

        assert!(
            !rows
                .iter()
                .any(|row| row.path == target || row.path.starts_with(&target)),
            "排除名单命中的目录及其子项都不应出现"
        );
        assert!(rows.iter().any(|row| row.path == visible));
    }

    #[test]
    fn ignored_directories_do_not_expand_and_files_are_marked() {
        // 忽略信息来自 git 状态（FileStatus::Ignored），由 Project 注入。
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let node_modules = directory.path().join("node_modules");
        std::fs::create_dir_all(node_modules.join("pkg")).expect("应创建被忽略目录");
        std::fs::write(node_modules.join("pkg").join("index.js"), "// ignored")
            .expect("应创建被忽略文件");
        std::fs::write(directory.path().join("app.log"), "log").expect("应创建日志文件");
        let visible = directory.path().join("main.js");
        std::fs::write(&visible, "console.log(1)").expect("应创建可见文件");

        let worktree = Worktree::new(directory.path().to_path_buf());
        let statuses = HashMap::from([
            (node_modules.clone(), FileStatus::Ignored),
            (directory.path().join("app.log"), FileStatus::Ignored),
        ]);
        let expanded = HashSet::from([directory.path().to_path_buf(), node_modules.clone()]);
        let rows = worktree.visible_entries(&expanded, git_status_from(&statuses));

        let nm = rows
            .iter()
            .find(|row| row.path == node_modules)
            .expect("node_modules 行应存在");
        assert!(matches!(nm.git_status, Some(FileStatus::Ignored)));
        assert!(
            !rows
                .iter()
                .any(|row| row.path.starts_with(&node_modules.join("pkg"))),
            "被忽略目录不应展开内容"
        );
        assert!(
            rows.iter()
                .find(|row| row.path == directory.path().join("app.log"))
                .is_some_and(|row| matches!(row.git_status, Some(FileStatus::Ignored))),
            "*.log 文件应被标记为忽略"
        );
        assert!(
            !rows
                .iter()
                .find(|row| row.path == visible)
                .expect("可见文件行应存在")
                .git_status
                .is_some()
        );
    }

    #[test]
    fn nested_gitignore_applies_within_its_directory() {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let sub = directory.path().join("sub");
        let nested = sub.join("secret.txt");
        std::fs::create_dir_all(&sub).expect("应创建子目录");
        std::fs::write(sub.join(".gitignore"), "secret.txt\n").expect("应写嵌套 .gitignore");
        std::fs::write(&nested, "secret").expect("应创建被忽略文件");
        let visible = sub.join("visible.txt");
        std::fs::write(&visible, "visible").expect("应创建可见文件");
        std::fs::write(directory.path().join(".gitignore"), "*.log\n").expect("应写根 .gitignore");

        let worktree = Worktree::new(directory.path().to_path_buf());
        let statuses = HashMap::from([
            (nested.clone(), FileStatus::Ignored),
            (visible.clone(), FileStatus::Untracked),
        ]);
        let expanded = HashSet::from([directory.path().to_path_buf(), sub.clone()]);
        let rows = worktree.visible_entries(&expanded, git_status_from(&statuses));

        assert!(
            rows.iter()
                .find(|row| row.path == nested)
                .expect("secret.txt 行应存在")
                .git_status
                .is_some_and(|status| matches!(status, FileStatus::Ignored)),
            "嵌套目录的忽略规则应生效"
        );
        assert!(
            !rows
                .iter()
                .find(|row| row.path == visible)
                .expect("visible.txt 行应存在")
                .git_status
                .is_some_and(|status| matches!(status, FileStatus::Ignored))
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

    // ── git 仓库发现 ─────────────────────────────────────────────

    /// 创建带一个初始提交的临时 git 仓库，返回 (仓库根, 目录句柄)。
    fn test_repo() -> (PathBuf, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let root = temp_dir.path().to_path_buf();
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        std::fs::write(root.join("tracked.txt"), "第一行\n第二行\n").expect("应写入初始文件");
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
    fn discovers_repository_from_any_ancestor() {
        let (root, _temp) = test_repo();
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
        let (root, _temp) = test_repo();
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
        let (root, _temp) = test_repo();
        std::fs::create_dir_all(root.join("nested")).expect("应创建嵌套目录");
        run_in(&root.join("nested"), &["git", "init", "-q"]);
        std::fs::create_dir_all(root.join("node_modules/pkg")).expect("应创建依赖目录");
        run_in(&root.join("node_modules/pkg"), &["git", "init", "-q"]);

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
}
