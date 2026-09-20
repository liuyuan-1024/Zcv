//! Worktree —— 项目目录快照层。
//!
//! 职责边界：目录遍历、扫描排除规则、git 仓库发现与路径命名语义住在这一层；
//! git 状态由项目树从 `GitStore` 的不可变快照按需派生。
//! 本层只提供静态目录查询（`children`），展开、深度与可见行是项目树视图状态，由 UI 层自行构建。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use zcv_git::{GitRepository, RealGitRepository};
use zcv_path::AbsolutePathBuf;

/// `.git` 目录名（仓库发现用）。
const DOT_GIT: &str = ".git";

/// 目录快照层的静态条目：由 Worktree 遍历产出，不含展开/深度等视图状态。
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    pub path: AbsolutePathBuf,
    pub name: String,
    pub is_dir: bool,
}

/// 项目目录快照层：持有根路径与扫描排除规则，提供静态目录查询。
pub(crate) struct Worktree {
    root: AbsolutePathBuf,
    filter: TreeFilter,
}

#[derive(Clone)]
pub(crate) struct WorktreeSearchPlan {
    pub(crate) root: AbsolutePathBuf,
    filter: TreeFilter,
}

impl Worktree {
    pub(crate) fn new(root: AbsolutePathBuf) -> Self {
        Self {
            root,
            filter: TreeFilter::new(&[]),
        }
    }

    /// 更换项目根目录（展开与选中状态由 UI 层重置，本层只换根）。
    pub(crate) fn set_root(&mut self, root: AbsolutePathBuf) {
        self.root = root;
    }

    /// 更新扫描排除规则（设置变化时由 Project 调用）。
    pub(crate) fn set_exclusions(&mut self, exclusions: &[String]) {
        self.filter = TreeFilter::new(exclusions);
    }

    /// 当前过滤规则的克隆（后台可见行收集任务捕获用）。
    pub(crate) fn filter(&self) -> TreeFilter {
        self.filter.clone()
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
pub(crate) struct TreeFilter {
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

/// 读取 `dir` 的直接子项（纯函数，可在后台线程执行）：目录优先、名称升序，扫描排除名单命中即过滤。
///
/// `collect_visible_entries` 逐层递归复用本函数，排序与排除规则天然一致。
fn children_sorted(dir: &Path, root: &Path, filter: &TreeFilter) -> Vec<WorktreeEntry> {
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let is_dir = path.is_dir();
                Some((path, is_dir))
            })
            .collect(),
        Err(_) => return Vec::new(),
    };
    entries.sort_by(|a, b| {
        if a.1 != b.1 {
            b.1.cmp(&a.1)
        } else {
            a.0.file_name().cmp(&b.0.file_name())
        }
    });
    entries
        .into_iter()
        .filter_map(|(path, is_dir)| {
            let name = path.file_name()?.to_string_lossy().to_string();
            // 扫描排除名单命中的条目根本不加载。
            let rel = path.strip_prefix(root).ok()?;
            if filter.is_excluded(rel) {
                return None;
            }
            Some(WorktreeEntry {
                path: AbsolutePathBuf::new(path).ok()?,
                name,
                is_dir,
            })
        })
        .collect()
}

/// 收集可见行（纯函数，可在后台线程执行）：根行 + 按 `expanded` 递归展开。
///
/// 排序与排除规则与 `Worktree::children` 一致；
/// 不含 git 状态；GitStore 快照由项目树在行模型应用后单独查询。
/// 展开、深度与选中是视图状态，由 UI 层决定。
pub(crate) fn collect_visible_entries(
    root: &AbsolutePathBuf,
    expanded: &HashSet<AbsolutePathBuf>,
    filter: &TreeFilter,
) -> Vec<WorktreeEntry> {
    let root_name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| root.to_string_lossy().to_string());
    let mut rows = vec![WorktreeEntry {
        path: root.clone(),
        name: root_name,
        is_dir: true,
    }];
    if expanded.contains(root) {
        collect_expanded_children(root.as_path(), root.as_path(), expanded, filter, &mut rows);
    }
    rows
}

/// 递归收集目录下已展开的子项（`collect_visible_entries` 的递归体）。
fn collect_expanded_children(
    dir: &Path,
    root: &Path,
    expanded: &HashSet<AbsolutePathBuf>,
    filter: &TreeFilter,
    rows: &mut Vec<WorktreeEntry>,
) {
    for entry in children_sorted(dir, root, filter) {
        if entry.is_dir && expanded.contains(&entry.path) {
            let path = entry.path.clone();
            rows.push(entry);
            collect_expanded_children(path.as_path(), root, expanded, filter, rows);
        } else {
            rows.push(entry);
        }
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
#[path = "test/worktree_tests.rs"]
mod tests;
