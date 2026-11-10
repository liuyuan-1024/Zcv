//! Worktree —— 项目目录快照层。
//!
//! 职责边界：目录遍历、扫描排除规则、git 仓库发现与路径命名语义住在这一层；
//! 已加载目录的条目由 Worktree 持有，文件事件使受影响目录失效，后台核对后更新快照。
//! git 状态与忽略标志由 `Project` 在收集可见行时从 `GitStore` 的不可变快照派生。
//! 展开、深度与选中仍属于项目树视图状态。

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use zcv_git::{GitRepository, RealGitRepository};
use zcv_path::{AbsolutePathBuf, ProjectRootDisplay, ProjectRootId};

/// `.git` 目录名（仓库发现用）。
const DOT_GIT: &str = ".git";

/// 目录快照层的静态条目：由 Worktree 遍历产出，不含展开/深度等视图状态。
///
/// `is_ignored` 由 `Project` 在收集可见行时从 Git 状态快照派生并写入，与条目的 Git 状态色相互独立。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: AbsolutePathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_ignored: bool,
}

/// 项目目录快照层：持有根路径、扫描排除规则和已加载目录的条目。
pub(crate) struct Worktree {
    root: AbsolutePathBuf,
    filter: TreeFilter,
    children: HashMap<AbsolutePathBuf, Vec<WorktreeEntry>>,
    dirty: HashSet<AbsolutePathBuf>,
}

#[derive(Clone)]
pub(crate) struct WorktreeSearchPlan {
    pub(crate) root_id: ProjectRootId,
    pub(crate) root: AbsolutePathBuf,
    pub(crate) root_display: ProjectRootDisplay,
    filter: TreeFilter,
    /// 嵌套在本根内部的其它项目根；外层扫描不得重复产出这些根下的文件。
    nested_roots: Vec<AbsolutePathBuf>,
}

impl Worktree {
    pub(crate) fn new(root: AbsolutePathBuf) -> Self {
        Self {
            root,
            filter: TreeFilter::new(&[]),
            children: HashMap::new(),
            dirty: HashSet::new(),
        }
    }

    /// 更换项目根目录并清除旧根的条目快照；展开与选中状态由项目树迁移。
    pub(crate) fn set_root(&mut self, root: AbsolutePathBuf) {
        self.root = root;
        self.children.clear();
        self.dirty.clear();
    }

    /// 更新扫描排除规则（设置变化时由 Project 调用）。
    pub(crate) fn set_exclusions(&mut self, exclusions: &[String]) {
        self.filter = TreeFilter::new(exclusions);
        self.children.clear();
        self.dirty.clear();
    }

    /// 当前过滤规则的克隆（后台可见行收集任务捕获用）。
    pub(crate) fn filter(&self) -> TreeFilter {
        self.filter.clone()
    }

    pub(crate) fn directories_to_scan(
        &self,
        expanded: &HashSet<AbsolutePathBuf>,
    ) -> Vec<AbsolutePathBuf> {
        expanded
            .iter()
            .filter(|path| {
                self.includes_expanded_directory(path)
                    && (!self.children.contains_key(*path) || self.dirty.contains(*path))
            })
            .cloned()
            .collect()
    }

    pub(crate) fn expanded_directories_loaded(&self, expanded: &HashSet<AbsolutePathBuf>) -> bool {
        expanded
            .iter()
            .filter(|path| self.includes_expanded_directory(path))
            .all(|path| self.children.contains_key(path) && !self.dirty.contains(path))
    }

    fn includes_expanded_directory(&self, path: &AbsolutePathBuf) -> bool {
        path.strip_prefix(&self.root)
            .is_ok_and(|relative| !self.filter.is_excluded(relative))
    }

    pub(crate) fn invalidate(&mut self, path: &Path, recursive: bool) {
        if recursive {
            self.dirty.extend(
                self.children
                    .keys()
                    .filter(|directory| directory.starts_with(path))
                    .cloned(),
            );
        }
        let directory =
            AbsolutePathBuf::new(path.to_path_buf()).expect("项目内文件事件路径必须是绝对路径");
        if self.children.contains_key(&directory) {
            self.dirty.insert(directory);
        }
        for parent in path.ancestors().skip(1) {
            let directory =
                AbsolutePathBuf::new(parent.to_path_buf()).expect("项目目录祖先必须是绝对路径");
            if self.children.contains_key(&directory) {
                self.dirty.insert(directory);
                break;
            }
        }
    }

    pub(crate) fn mark_scanned_directory_dirty(&mut self, directory: AbsolutePathBuf) {
        self.dirty.insert(directory);
    }

    pub(crate) fn scan_failed(&mut self, directory: AbsolutePathBuf) {
        self.dirty.remove(&directory);
        self.children.entry(directory).or_default();
    }

    pub(crate) fn apply_scan(
        &mut self,
        directory: AbsolutePathBuf,
        entries: Vec<WorktreeEntry>,
    ) -> (bool, bool) {
        self.dirty.remove(&directory);
        let previous = self.children.insert(directory.clone(), entries);
        let current = &self.children[&directory];
        let changed = previous.as_ref() != Some(current);
        let existing_changed = previous.is_some() && changed;
        let removed_directories = previous
            .into_iter()
            .flatten()
            .filter(|old| {
                old.is_dir
                    && !current
                        .iter()
                        .any(|entry| entry.path == old.path && entry.is_dir)
            })
            .map(|entry| entry.path)
            .collect::<Vec<_>>();
        for removed in removed_directories {
            self.children.retain(|path, _| !path.starts_with(&removed));
            self.dirty.retain(|path| !path.starts_with(&removed));
        }
        (changed, existing_changed)
    }

    pub(crate) fn visible_entries(
        &self,
        expanded: &HashSet<AbsolutePathBuf>,
    ) -> Vec<WorktreeEntry> {
        collect_visible_entries(&self.root, expanded, &self.children)
    }
}

impl WorktreeSearchPlan {
    pub(crate) fn new(
        root_id: ProjectRootId,
        root: AbsolutePathBuf,
        root_display: ProjectRootDisplay,
        filter: TreeFilter,
        nested_roots: Vec<AbsolutePathBuf>,
    ) -> Self {
        Self {
            root_id,
            root,
            root_display,
            filter,
            nested_roots,
        }
    }
}

/// 目录扫描的排除规则（glob 名单）。
///
/// file_scan_exclusions 命中的条目根本不在行模型中加载；
/// 忽略（gitignore/info/exclude）仍由 Git 状态判定（`FileStatus::Ignored`），在收集可见行时投影为行模型的 `is_ignored`，不参与本过滤层。
#[derive(Clone)]
pub(crate) struct TreeFilter {
    /// 用户配置的扫描排除 glob。
    exclusions: GlobSet,
}

impl WorktreeSearchPlan {
    pub(crate) fn is_excluded(&self, path: &Path) -> bool {
        // 外层根跳过嵌套根的整个子树；嵌套根自己的扫描计划会产出这些文件。
        self.nested_roots
            .iter()
            .any(|nested| path.starts_with(nested.as_path()))
            || path
                .strip_prefix(&self.root)
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

/// 读取 `dir` 的直接子项（纯函数，可在后台线程执行）：目录优先、名称自然排序，扫描排除名单命中即过滤。
///
/// Worktree 后台扫描使用本函数，排序与排除规则天然一致。
pub(crate) fn children_sorted(
    dir: &Path,
    root: &Path,
    filter: &TreeFilter,
) -> io::Result<Vec<WorktreeEntry>> {
    let read_dir = match fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    let mut entries = read_dir
        .map(|entry| {
            let path = entry?.path();
            let is_dir = path.is_dir();
            Ok((path, is_dir))
        })
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort_by(|a, b| zcv_path::compare_tree_entries((&a.0, a.1), (&b.0, b.1)));
    Ok(entries
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
                is_ignored: false,
            })
        })
        .collect())
}

/// 工作区根集合中路径的所属根；嵌套根命中多个时取最长前缀（最内层根）。
///
/// 所有归属判断与展示相对化共用的唯一规则；
/// 持有 Project 实体的调用方应使用 `Project::root_for_path`。
pub fn owning_workspace_root<'a, R: AsRef<Path> + 'a + ?Sized>(
    path: &Path,
    roots: impl IntoIterator<Item = &'a R>,
) -> Option<&'a Path> {
    roots
        .into_iter()
        .map(AsRef::<Path>::as_ref)
        .filter(|root| path.starts_with(*root))
        .max_by_key(|root| root.as_os_str().len())
}

/// 从已加载条目收集可见行：根行 + 按 `expanded` 递归展开。
///
/// 排序与排除规则已在扫描时确定；Git 状态与忽略标志由 `Project` 在收集任务的同一次后台快照中合并。
/// 展开、深度与选中是视图状态，由 UI 层决定。
pub(crate) fn collect_visible_entries(
    root: &AbsolutePathBuf,
    expanded: &HashSet<AbsolutePathBuf>,
    children: &HashMap<AbsolutePathBuf, Vec<WorktreeEntry>>,
) -> Vec<WorktreeEntry> {
    let root_name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| root.to_string_lossy().to_string());
    let mut rows = vec![WorktreeEntry {
        path: root.clone(),
        name: root_name,
        is_dir: true,
        is_ignored: false,
    }];
    if expanded.contains(root) {
        collect_expanded_children(root, expanded, children, &mut rows);
    }
    rows
}

/// 递归收集目录下已展开的子项（`collect_visible_entries` 的递归体）。
fn collect_expanded_children(
    dir: &AbsolutePathBuf,
    expanded: &HashSet<AbsolutePathBuf>,
    children: &HashMap<AbsolutePathBuf, Vec<WorktreeEntry>>,
    rows: &mut Vec<WorktreeEntry>,
) {
    for entry in children.get(dir).into_iter().flatten() {
        if entry.is_dir && expanded.contains(&entry.path) {
            let path = entry.path.clone();
            rows.push(entry.clone());
            collect_expanded_children(&path, expanded, children, rows);
        } else {
            rows.push(entry.clone());
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
        for entry in fs::read_dir(dir)? {
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

/// 合并多个工作区根的仓库发现结果：逐根发现后按 working_directory 去重，保持首次出现顺序。
///
/// 多根工作区中同一仓库可能被多个根的发现路径覆盖（嵌套根、外层仓库）；
/// 去重依据与 `discover_repositories` 一致：working_directory 来自 canonicalize，比较天然一致。
pub(crate) fn discover_repositories_for_roots(
    roots: &[AbsolutePathBuf],
) -> anyhow::Result<Vec<RealGitRepository>> {
    let mut repositories = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for root in roots {
        for repository in discover_repositories(root.as_path())? {
            if seen.insert(repository.working_directory().to_path_buf()) {
                repositories.push(repository);
            }
        }
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
