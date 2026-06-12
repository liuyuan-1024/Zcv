//! 项目目录树（文件树面板的数据源）。
//!
//! 持有项目根、按需懒加载的目录子项缓存，以及目录展开状态。子项排序规则：
//! 目录优先、字母序（不区分大小写）。
//!
//! 这是一个面向只读浏览的快照层：`visible_rows` 给 UI 直接消费；`expand` / `collapse` / `toggle` 由命令侧调用。
//! 本层不负责文件内容、git 状态、watch 失效——这些由上层服务叠加，本模块只维护目录树快照。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// 项目级 zom 配置目录名。
///
/// 用户级配置在 `$HOME/.zom/`；项目级在 `项目根/.zom/`。
/// 两边同名，语义上「项目级覆盖用户级」——目前只用到 `.zomignore`，后续 `config.toml` 等
/// 项目级覆盖落地时复用本目录。
const PROJECT_CONFIG_DIR: &str = ".zom";
const ZOMIGNORE_FILE: &str = ".zomignore";
const GITIGNORE_FILE: &str = ".gitignore";

/// 首次为项目生成 `.zom/.zomignore` 时写入的默认内容。
///
/// 设计取向：「不想看到」的文件交给 `.gitignore` 维护（也确保它们不被提交）—— 它由 [`IgnoreMatcher`] 默认继承（与 Zed / VSCode 一致）。
/// `.zomignore` 的单一职责是**反向放开**：用 `!pattern` 把「不想提交、但想在编辑器里看到」的文件 / 目录从 `.gitignore` 的隐藏里取回。
/// 默认模板因此不含任何规则，只留注释说明用法。
const ZOMIGNORE_DEFAULT: &str = "\
# zom 文件树忽略规则（语法同 .gitignore）。
#
# 项目根的 .gitignore 已被默认继承——「不想看到」的文件请写在那边，顺便也避免被提交。
#
# 本文件的用途是反过来：用 `!pattern` 把被 .gitignore 隐藏、但你想在编辑器里看到的文件 / 目录放回文件树。

# 环境变量示例——团队共享，常被通配 .env*
!.env
!.env.*

# 空目录占位——容易被过度匹配的规则误伤
!.gitkeep
!.keep

# 构建产物——偶尔想浏览生成结果 / 检查产物结构。
# 文件树懒加载，不展开不会扫盘；node_modules 这种巨型依赖目录默认不放开。
!target/
!dist/
!build/
!out/
";

/// 节点类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    Directory,
    File,
}

/// 一个目录下的单个子项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
}

/// 用于渲染的一行。
///
/// 借用 [`ProjectTree`] 内部缓存，避免快照时复制大量 PathBuf。生命周期与产出它的 `visible_rows` 调用保持一致。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TreeRow<'a> {
    pub path: &'a Path,
    pub name: &'a str,
    pub depth: usize,
    pub kind: EntryKind,
    /// 仅对目录有意义；文件恒为 false。
    pub expanded: bool,
}

/// 项目目录树。
pub struct ProjectTree {
    root: PathBuf,
    /// 项目根的展示名（取 `root.file_name()`；空时回落到完整路径）。
    /// 缓存在结构体里，避免 `visible_rows` 借出 `&str` 时无处寄存。
    root_name: String,
    /// 已读取过的目录及其排序好的子项列表。
    children: HashMap<PathBuf, Vec<TreeEntry>>,
    /// 当前展开的目录路径集合。根目录默认在集合里。
    expanded: HashSet<PathBuf>,
    /// 项目根级别的忽略规则集，构造时一次性加载。
    /// `load_dir` 用它过滤掉每个目录下被忽略的子项。
    ignore: IgnoreMatcher,
}

impl ProjectTree {
    /// 新建并立刻加载根目录子项；根目录读取失败时返回 IO 错误。
    ///
    /// 构造期会确保 `项目根/.zom/.zomignore` 存在（首次打开新项目时自动写入 [`ZOMIGNORE_DEFAULT`]），
    /// 随后与 `项目根/.gitignore` 一起编译成[`IgnoreMatcher`]；
    /// 之后 `load_dir` 据此过滤每层子项。
    pub fn new(root: PathBuf) -> io::Result<Self> {
        let root_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        let ignore = IgnoreMatcher::for_root(&root)?;
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        let mut tree = Self {
            root: root.clone(),
            root_name,
            children: HashMap::new(),
            expanded,
            ignore,
        };
        tree.load_dir(&root)?;
        Ok(tree)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    /// 展开目录。
    ///
    /// 第一次展开时按需读盘；读盘失败的目录保持折叠并保留错误的语义（调用方拿到 `Err` 自行决定如何提示）。
    pub fn expand(&mut self, path: &Path) -> io::Result<()> {
        self.ensure_loaded(path)?;
        self.expanded.insert(path.to_path_buf());
        Ok(())
    }

    pub fn collapse(&mut self, path: &Path) {
        self.expanded.remove(path);
    }

    pub fn toggle(&mut self, path: &Path) -> io::Result<()> {
        if self.is_expanded(path) {
            self.collapse(path);
            Ok(())
        } else {
            self.expand(path)
        }
    }

    /// 在 `parent` 目录下创建一个文件 / 子目录，并刷新目录缓存，使 `visible_rows` 立即反映新条目。
    /// 返回新条目的完整路径。
    /// `name` 可以是相对路径；
    /// 创建文件时会按需创建中间目录，创建目录时会创建整段目录路径。
    ///
    /// 同名条目已存在时返回 [`io::ErrorKind::AlreadyExists`]，不会覆盖。
    pub fn create_entry(
        &mut self,
        parent: &Path,
        name: &str,
        kind: EntryKind,
    ) -> io::Result<PathBuf> {
        let relative = validate_relative_entry_path(name)?;
        let path = parent.join(relative);
        match kind {
            EntryKind::File => {
                if let Some(target_parent) = path.parent() {
                    ensure_directory_path_available(parent, target_parent)?;
                    fs::create_dir_all(target_parent)?;
                }
                fs::File::create_new(&path)?;
            }
            EntryKind::Directory => {
                if path.exists() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("条目已存在：{}", path.display()),
                    ));
                }
                if let Some(target_parent) = path.parent() {
                    ensure_directory_path_available(parent, target_parent)?;
                }
                fs::create_dir_all(&path)?;
            }
        }
        self.reload_expanded_dirs()?;
        Ok(path)
    }

    /// 把 `path` 移入系统回收站，并刷新其父目录缓存，使 `visible_rows` 立即不再含该条目。
    ///
    /// 用「移入回收站」而非永久删除：可在系统层面恢复，降低误删代价。
    /// 文件与目录皆可：目录连同其全部内容一并移入回收站。
    pub fn delete_entry(&mut self, path: &Path) -> io::Result<()> {
        trash::delete(path).map_err(io::Error::other)?;
        self.reload_parent(path)
    }

    /// 永久删除 `path`，并刷新其父目录缓存。
    ///
    /// 真实用户操作应优先走 [`delete_entry`](Self::delete_entry) 移入系统回收站；
    /// 这个入口用于不适合依赖系统 Trash/Finder 的测试与内部搬移场景。
    pub fn delete_entry_permanently(&mut self, path: &Path) -> io::Result<()> {
        remove_recursive(path)?;
        self.reload_parent(path)
    }

    fn reload_parent(&mut self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            self.children.remove(parent);
            self.load_dir(parent)?;
        }
        Ok(())
    }

    /// 把 `src` 复制到 `dst_parent` 下。新条目的名称默认取 `src` 的文件名；
    /// 目标目录已有同名时自动追加 ` (n)` 后缀直到找到空位，**永不覆盖**。
    /// 文件直接 `fs::copy`，目录递归复制。返回最终落点的绝对路径。
    ///
    /// 拒绝把 `src` 复制到自身或自己的子目录（会形成无限递归）；
    /// 找不到 `src` 或 `dst_parent` 不是目录时返回相应 IO 错误。
    ///
    /// 跨进程粘贴的「来自外部 Finder」也走这条路径——`src` 不必在项目根之内。
    pub fn copy_entry(&mut self, src: &Path, dst_parent: &Path) -> io::Result<PathBuf> {
        let final_dst = prepare_paste_destination(src, dst_parent)?;
        copy_recursive(src, &final_dst)?;
        self.reload_expanded_dirs()?;
        Ok(final_dst)
    }

    /// 把 `src` 移动到 `dst_parent` 下。命名规则与 [`copy_entry`](Self::copy_entry) 一致：
    /// 默认沿用 `src` 文件名、冲突自动改名、永不覆盖。返回新位置。
    ///
    /// 同源同父目录的「移动」是无操作（早返），避免触发自动改名得到 `foo (1).txt` 这种用户没要的结果。
    ///
    /// 实现优先用 `fs::rename`（同盘符瞬时），失败时回退到 `copy_recursive + remove_recursive`，覆盖跨文件系统 / 跨盘的情形。
    /// 回退路径里若 copy 或 remove 自身也失败，原 rename 错误会被抛出，让调用方至少知道操作未完成。
    pub fn move_entry(&mut self, src: &Path, dst_parent: &Path) -> io::Result<PathBuf> {
        if src.parent() == Some(dst_parent) {
            // 同父目录 + 同名：无操作；不调用 pick_unique_name，避免误改名。
            return Ok(src.to_path_buf());
        }
        let final_dst = prepare_paste_destination(src, dst_parent)?;
        if let Err(rename_err) = fs::rename(src, &final_dst) {
            // 跨文件系统等场景：copy 再 remove。两步任一失败，抛出原 rename 错误，
            // 让调用方据此提示用户而不是看到一个误导性的次生错误。
            if copy_recursive(src, &final_dst)
                .and_then(|_| remove_recursive(src))
                .is_err()
            {
                return Err(rename_err);
            }
        }
        self.reload_expanded_dirs()?;
        Ok(final_dst)
    }

    /// 把 `path` 在原父目录内改名为 `new_name`。
    /// `new_name` 必须是单段名字（不含路径分隔符、不含 `.` / `..`），与原名相同视为无操作。
    /// 同父目录下已存在同名项时返回 [`io::ErrorKind::AlreadyExists`]——**永不覆盖**，由调用方提示。
    /// 返回新路径。
    pub fn rename_entry(&mut self, path: &Path, new_name: &str) -> io::Result<PathBuf> {
        if path == self.root {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "不能重命名项目根",
            ));
        }
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "新名称不能为空",
            ));
        }
        if trimmed.contains('/') || trimmed.contains('\\') || trimmed == "." || trimmed == ".." {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("无效的名称：{trimmed}"),
            ));
        }
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "条目没有父目录，无法重命名")
        })?;
        let dst = parent.join(trimmed);
        if dst == path {
            return Ok(path.to_path_buf());
        }
        if dst.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("同名条目已存在：{}", dst.display()),
            ));
        }
        fs::rename(path, &dst)?;
        // expanded 里凡是落在 path 之下（含 path 自身）的目录都要 rebase，
        // 否则 reload_expanded_dirs 会去读已经不存在的旧路径而把它们悄悄丢掉。
        let rebased: Vec<(PathBuf, PathBuf)> = self
            .expanded
            .iter()
            .filter_map(|expanded| {
                expanded
                    .strip_prefix(path)
                    .ok()
                    .map(|rest| (expanded.clone(), dst.join(rest)))
            })
            .collect();
        for (old, new) in rebased {
            self.expanded.remove(&old);
            self.expanded.insert(new);
        }
        self.reload_expanded_dirs()?;
        Ok(dst)
    }

    /// 自根向下做 DFS，按目录优先 + 字母序产出可见行。
    ///
    /// 注意：返回值借用 `&self`，调用 `expand`/`collapse` 前必须先把它丢弃。
    /// 项目根本身占第一行（depth=0）；其子项在 depth=1。
    pub fn visible_rows(&self) -> Vec<TreeRow<'_>> {
        let mut rows = Vec::new();
        let root_expanded = self.expanded.contains(&self.root);
        rows.push(TreeRow {
            path: &self.root,
            name: &self.root_name,
            depth: 0,
            kind: EntryKind::Directory,
            expanded: root_expanded,
        });
        if root_expanded {
            self.collect_visible(&self.root, 1, &mut rows);
        }
        rows
    }

    fn collect_visible<'a>(&'a self, dir: &Path, depth: usize, rows: &mut Vec<TreeRow<'a>>) {
        let Some(entries) = self.children.get(dir) else {
            return;
        };
        for entry in entries {
            let expanded =
                matches!(entry.kind, EntryKind::Directory) && self.expanded.contains(&entry.path);
            rows.push(TreeRow {
                path: &entry.path,
                name: &entry.name,
                depth,
                kind: entry.kind,
                expanded,
            });
            if expanded {
                self.collect_visible(&entry.path, depth + 1, rows);
            }
        }
    }

    fn ensure_loaded(&mut self, dir: &Path) -> io::Result<()> {
        if self.children.contains_key(dir) {
            return Ok(());
        }
        self.load_dir(dir)
    }

    fn load_dir(&mut self, dir: &Path) -> io::Result<()> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            // symlink 暂按其目标类型对待：is_dir / is_file 会跟随。
            // 后续若要单独标识 symlink，再扩 EntryKind。
            let kind = if file_type.is_dir() {
                EntryKind::Directory
            } else {
                EntryKind::File
            };
            let entry_path = entry.path();
            if self
                .ignore
                .is_ignored(&entry_path, matches!(kind, EntryKind::Directory))
            {
                continue;
            }
            entries.push(TreeEntry {
                path: entry_path,
                name: entry.file_name().to_string_lossy().into_owned(),
                kind,
            });
        }
        entries.sort_by(|a, b| match (a.kind, b.kind) {
            (EntryKind::Directory, EntryKind::File) => std::cmp::Ordering::Less,
            (EntryKind::File, EntryKind::Directory) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });
        self.children.insert(dir.to_path_buf(), entries);
        Ok(())
    }

    fn reload_expanded_dirs(&mut self) -> io::Result<()> {
        let mut expanded: Vec<_> = self.expanded.iter().cloned().collect();
        expanded.sort_by_key(|path| path.components().count());
        self.children.clear();
        for dir in expanded {
            if dir.is_dir() {
                self.load_dir(&dir)?;
            }
        }
        Ok(())
    }
}

/// 项目级 ignore 规则集：把 `项目根/.gitignore` 与 `项目根/.zom/.zomignore` 编译成单个 [`Gitignore`] matcher。
///
/// 加载顺序：`.gitignore` → `.zomignore`。`Gitignore` 内部按"最后匹配胜出" 的 gitignore 语义解析，
/// 所以 `.zomignore` 里的 `!pattern` 能覆盖 `.gitignore` 里的忽略——这是用户「想看 `dist/` 某个子目录」的标准入口。
///
/// 本结构只读根级两个文件。子目录里的 `.gitignore` 暂不递归继承——多数项目把 ignore 集中在根，简化实现；
/// 后续需要时再加层级累积。
struct IgnoreMatcher {
    matcher: Gitignore,
}

impl IgnoreMatcher {
    fn for_root(root: &Path) -> io::Result<Self> {
        ensure_zomignore_exists(root)?;
        let mut builder = GitignoreBuilder::new(root);
        let gitignore_path = root.join(GITIGNORE_FILE);
        if gitignore_path.is_file() {
            if let Some(error) = builder.add(&gitignore_path) {
                return Err(io::Error::other(format!(
                    "解析 {} 失败：{error}",
                    gitignore_path.display()
                )));
            }
        }
        let zomignore_path = root.join(PROJECT_CONFIG_DIR).join(ZOMIGNORE_FILE);
        if let Some(error) = builder.add(&zomignore_path) {
            return Err(io::Error::other(format!(
                "解析 {} 失败：{error}",
                zomignore_path.display()
            )));
        }
        let matcher = builder
            .build()
            .map_err(|error| io::Error::other(format!("构建忽略规则失败：{error}")))?;
        Ok(Self { matcher })
    }

    fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        matches!(self.matcher.matched(path, is_dir), Match::Ignore(_))
    }
}

/// 首次为项目生成 `.zom/.zomignore`：若文件已存在则跳过（**不覆盖用户编辑**），否则按需建目录并写入 [`ZOMIGNORE_DEFAULT`]。
/// 父目录是只读等场景下返回 IO 错误，由 [`ProjectTree::new`] 透传给上层。
fn ensure_zomignore_exists(root: &Path) -> io::Result<()> {
    let dir = root.join(PROJECT_CONFIG_DIR);
    let path = dir.join(ZOMIGNORE_FILE);
    if path.exists() {
        return Ok(());
    }
    fs::create_dir_all(&dir)?;
    fs::write(&path, ZOMIGNORE_DEFAULT)
}

fn validate_relative_entry_path(raw: &str) -> io::Result<PathBuf> {
    let path = Path::new(raw);
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("无效的相对路径：{raw}"),
                ));
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "入口路径不能为空",
        ));
    }
    Ok(relative)
}

fn ensure_directory_path_available(root: &Path, target: &Path) -> io::Result<()> {
    let relative = target.strip_prefix(root).unwrap_or(target);
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                current.push(part);
                if current.exists() && !current.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        format!(
                            "路径中的 {} 已是文件，无法作为目录使用",
                            part.to_string_lossy()
                        ),
                    ));
                }
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("无效的目录路径：{}", target.display()),
                ));
            }
        }
    }
    Ok(())
}

/// 给「粘贴到 `dst_parent`」挑一个最终路径：验完基本边界后，按 `src` 的文件名取一个未占用的名字。
/// **不**做任何文件 IO 写入。
fn prepare_paste_destination(src: &Path, dst_parent: &Path) -> io::Result<PathBuf> {
    if !src.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("源不存在：{}", src.display()),
        ));
    }
    if !dst_parent.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("目标目录不存在：{}", dst_parent.display()),
        ));
    }
    if !dst_parent.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("目标不是目录：{}", dst_parent.display()),
        ));
    }
    // 把目录粘贴到自身 / 自己内部会形成"自吞"。
    // Path::starts_with 是按组件匹配的，所以 /a/b 与 /a/bc 不会误判。
    if dst_parent == src || dst_parent.starts_with(src) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "不能粘贴到自身或其子目录：src={} dst_parent={}",
                src.display(),
                dst_parent.display()
            ),
        ));
    }
    let desired = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "源路径没有文件名"))?;
    let name = pick_unique_name(dst_parent, &desired);
    Ok(dst_parent.join(name))
}

/// 在 `parent` 目录里挑一个未被占用的文件名。无冲突直接返回 `desired`；
/// 冲突时按 `foo (1).txt`、`foo (2).txt` 顺序探测。
/// 万一全占用，兜底返回带纳秒时间戳的名字——这条几乎不会触发，但保证函数永远有返回值。
fn pick_unique_name(parent: &Path, desired: &str) -> String {
    if !parent.join(desired).exists() {
        return desired.to_string();
    }
    let path = Path::new(desired);
    // file_stem / extension 是按「最后一个点」切的：foo.tar.gz → stem=foo.tar、ext=gz。
    // 结果是 "foo.tar (1).gz"——和 macOS Finder 行为一致。
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned());
    for n in 1..=10_000 {
        let candidate = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        if !parent.join(&candidate).exists() {
            return candidate;
        }
    }
    // 兜底：一万条都占的情况，退化到纳秒时间戳。
    // 理论极端，实践中不会撞。
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    match ext {
        Some(e) => format!("{stem} ({ts}).{e}"),
        None => format!("{stem} ({ts})"),
    }
}

/// 递归复制：文件走 `fs::copy`（跟随 symlink，与 cp / Finder 一致），目录走 `create_dir + read_dir` 的递归。
/// 失败时已写入的副本不回滚——上层若需原子语义，应自行清理。
fn copy_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(src)?;
    if metadata.file_type().is_dir() {
        fs::create_dir(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let child_dst = dst.join(entry.file_name());
            copy_recursive(&entry.path(), &child_dst)?;
        }
        Ok(())
    } else {
        fs::copy(src, dst).map(|_| ())
    }
}

/// 递归删除：目录走 `remove_dir_all`，文件 / 符号链接走 `remove_file`。
/// 区别于 [`delete_entry`](ProjectTree::delete_entry) —— 这里是真删，不走回收站，仅用于 `move_entry` 的跨文件系统路径（先 copy 出去再删源）。
fn remove_recursive(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, create_dir_all};
    use std::path::PathBuf;

    fn tmp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zom-project-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn root_display_name(root: &Path) -> String {
        root.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned())
    }

    #[test]
    fn root_should_render_as_first_row_and_subdirs_should_load_on_expand() {
        let root = tmp_root("lazy");
        create_dir_all(root.join("src/inner")).unwrap();
        File::create(root.join("README.md")).unwrap();
        File::create(root.join("src/lib.rs")).unwrap();
        File::create(root.join("src/inner/mod.rs")).unwrap();

        let mut tree = ProjectTree::new(root.clone()).unwrap();
        let root_name = root_display_name(&root);
        let rows: Vec<_> = tree
            .visible_rows()
            .into_iter()
            .map(|row| (row.name.to_string(), row.depth, row.kind, row.expanded))
            .collect();
        // 根行 depth=0 默认展开；其下 depth=1（目录优先 + 字母序）。
        // `.zom/` 是 ProjectTree::new 自动创建的项目配置目录，与其他目录同列。
        assert_eq!(
            rows,
            vec![
                (root_name.clone(), 0, EntryKind::Directory, true),
                (".zom".to_string(), 1, EntryKind::Directory, false),
                ("src".to_string(), 1, EntryKind::Directory, false),
                ("README.md".to_string(), 1, EntryKind::File, false),
            ]
        );

        tree.expand(&root.join("src")).unwrap();
        let rows: Vec<_> = tree
            .visible_rows()
            .into_iter()
            .map(|row| (row.name.to_string(), row.depth))
            .collect();
        assert_eq!(
            rows,
            vec![
                (root_name.clone(), 0),
                (".zom".to_string(), 1),
                ("src".to_string(), 1),
                ("inner".to_string(), 2),
                ("lib.rs".to_string(), 2),
                ("README.md".to_string(), 1),
            ]
        );

        tree.collapse(&root.join("src"));
        // root + .zom + src + README.md
        assert_eq!(tree.visible_rows().len(), 4);

        // 折叠根目录后只剩根这一行。
        tree.collapse(&root);
        assert_eq!(tree.visible_rows().len(), 1);
    }

    #[test]
    fn create_entry_should_write_to_disk_and_refresh_rows() {
        let root = tmp_root("create");
        let mut tree = ProjectTree::new(root.clone()).unwrap();

        let file = tree
            .create_entry(&root, "new.txt", EntryKind::File)
            .unwrap();
        assert!(file.is_file());
        let dir = tree
            .create_entry(&root, "newdir", EntryKind::Directory)
            .unwrap();
        assert!(dir.is_dir());

        let names: Vec<_> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.name.to_string())
            .collect();
        assert!(names.contains(&"new.txt".to_string()));
        assert!(names.contains(&"newdir".to_string()));

        // 重复创建不覆盖，报 AlreadyExists。
        assert_eq!(
            tree.create_entry(&root, "new.txt", EntryKind::File)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn create_entry_should_create_nested_paths_without_overwriting() {
        let root = tmp_root("create-nested");
        let mut tree = ProjectTree::new(root.clone()).unwrap();

        let file = tree
            .create_entry(&root, "src/components/button.rs", EntryKind::File)
            .unwrap();
        assert!(file.is_file());
        assert!(root.join("src/components").is_dir());

        let dir = tree
            .create_entry(&root, "src/views/home", EntryKind::Directory)
            .unwrap();
        assert!(dir.is_dir());

        assert_eq!(
            tree.create_entry(&root, "../outside.txt", EntryKind::File)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            tree.create_entry(&root, "src/views/home", EntryKind::Directory)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );

        File::create(root.join("nihao")).unwrap();
        let error = tree
            .create_entry(&root, "nihao/ni/hao/ni", EntryKind::File)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotADirectory);
        assert_eq!(
            error.to_string(),
            "路径中的 nihao 已是文件，无法作为目录使用"
        );
    }

    #[test]
    fn copy_entry_should_copy_file_to_target_directory() {
        let root = tmp_root("copy-file");
        create_dir_all(root.join("a")).unwrap();
        create_dir_all(root.join("b")).unwrap();
        let src = root.join("a/note.txt");
        std::fs::write(&src, b"hello").unwrap();
        let mut tree = ProjectTree::new(root.clone()).unwrap();
        // 让 a / b 都被加载，使复制后的目录刷新能命中。
        tree.expand(&root.join("a")).unwrap();
        tree.expand(&root.join("b")).unwrap();

        let dst = tree.copy_entry(&src, &root.join("b")).unwrap();
        assert_eq!(dst, root.join("b/note.txt"));
        assert!(src.is_file(), "源文件应保留");
        assert_eq!(std::fs::read(&dst).unwrap(), b"hello");
    }

    #[test]
    fn copy_entry_should_copy_directory_recursively() {
        let root = tmp_root("copy-dir");
        create_dir_all(root.join("src/inner")).unwrap();
        create_dir_all(root.join("dst")).unwrap();
        std::fs::write(root.join("src/a.txt"), b"a").unwrap();
        std::fs::write(root.join("src/inner/b.txt"), b"b").unwrap();
        let mut tree = ProjectTree::new(root.clone()).unwrap();

        let dst = tree
            .copy_entry(&root.join("src"), &root.join("dst"))
            .unwrap();
        assert_eq!(dst, root.join("dst/src"));
        // 嵌套内容保留。
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"a");
        assert_eq!(std::fs::read(dst.join("inner/b.txt")).unwrap(), b"b");
        // 源保留。
        assert!(root.join("src/a.txt").is_file());
    }

    #[test]
    fn copy_entry_should_auto_rename_on_conflict() {
        let root = tmp_root("copy-rename");
        std::fs::write(root.join("note.txt"), b"v1").unwrap();
        let mut tree = ProjectTree::new(root.clone()).unwrap();

        // 同目录复制：会自动改名 "note (1).txt"。
        let dst1 = tree.copy_entry(&root.join("note.txt"), &root).unwrap();
        assert_eq!(dst1, root.join("note (1).txt"));
        assert!(dst1.is_file());

        // 再来一次："note (2).txt"。
        let dst2 = tree.copy_entry(&root.join("note.txt"), &root).unwrap();
        assert_eq!(dst2, root.join("note (2).txt"));

        // 无扩展名的情形：".." 之后追加 " (n)"。
        std::fs::write(root.join("README"), b"r").unwrap();
        let dst3 = tree.copy_entry(&root.join("README"), &root).unwrap();
        assert_eq!(dst3, root.join("README (1)"));
    }

    #[test]
    fn move_entry_should_relocate_and_refresh_rows() {
        let root = tmp_root("move-basic");
        create_dir_all(root.join("a")).unwrap();
        create_dir_all(root.join("b")).unwrap();
        std::fs::write(root.join("a/note.txt"), b"hi").unwrap();
        let mut tree = ProjectTree::new(root.clone()).unwrap();
        tree.expand(&root.join("a")).unwrap();
        tree.expand(&root.join("b")).unwrap();

        let dst = tree
            .move_entry(&root.join("a/note.txt"), &root.join("b"))
            .unwrap();
        assert_eq!(dst, root.join("b/note.txt"));
        assert!(!root.join("a/note.txt").exists());
        assert!(root.join("b/note.txt").is_file());

        // 刷新后的可见行应包含 b/note.txt。
        let names: Vec<_> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.name.to_string())
            .collect();
        assert!(names.contains(&"note.txt".to_string()));
    }

    #[test]
    fn move_entry_to_same_parent_should_be_noop() {
        let root = tmp_root("move-noop");
        std::fs::write(root.join("note.txt"), b"hi").unwrap();
        let mut tree = ProjectTree::new(root.clone()).unwrap();

        let dst = tree.move_entry(&root.join("note.txt"), &root).unwrap();
        // 与源同位置；不应触发自动改名得到 "note (1).txt"。
        assert_eq!(dst, root.join("note.txt"));
        assert!(root.join("note.txt").is_file());
        assert!(!root.join("note (1).txt").exists());
    }

    #[test]
    fn move_entry_into_self_or_descendant_should_error() {
        let root = tmp_root("move-into-self");
        create_dir_all(root.join("dir/inner")).unwrap();
        let mut tree = ProjectTree::new(root.clone()).unwrap();

        // 移到自身。
        let err = tree
            .move_entry(&root.join("dir"), &root.join("dir"))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

        // 移到自己的子目录。
        let err = tree
            .move_entry(&root.join("dir"), &root.join("dir/inner"))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

        // 源目录依然完好。
        assert!(root.join("dir/inner").is_dir());
    }

    #[test]
    fn copy_entry_into_self_or_descendant_should_error() {
        let root = tmp_root("copy-into-self");
        create_dir_all(root.join("dir/inner")).unwrap();
        std::fs::write(root.join("dir/note.txt"), b"hi").unwrap();
        let mut tree = ProjectTree::new(root.clone()).unwrap();

        let err = tree
            .copy_entry(&root.join("dir"), &root.join("dir/inner"))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn pick_unique_name_should_handle_dotted_filenames() {
        // 单测私有函数：foo.tar.gz → 冲突时 "foo.tar (1).gz"。
        let root = tmp_root("pick-unique");
        std::fs::write(root.join("foo.tar.gz"), b"x").unwrap();
        let picked = pick_unique_name(&root, "foo.tar.gz");
        assert_eq!(picked, "foo.tar (1).gz");

        // 无扩展名。
        std::fs::write(root.join("Makefile"), b"x").unwrap();
        let picked = pick_unique_name(&root, "Makefile");
        assert_eq!(picked, "Makefile (1)");

        // 无冲突原样返回。
        let picked = pick_unique_name(&root, "fresh.txt");
        assert_eq!(picked, "fresh.txt");
    }

    #[test]
    fn entries_should_be_sorted_dirs_first_then_alpha() {
        let root = tmp_root("sort");
        create_dir_all(root.join("b_dir")).unwrap();
        create_dir_all(root.join("A_dir")).unwrap();
        File::create(root.join("zfile")).unwrap();
        File::create(root.join("Afile")).unwrap();

        let tree = ProjectTree::new(root.clone()).unwrap();
        let names: Vec<_> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.name.to_string())
            .collect();
        // 目录优先 + 字母序（不区分大小写）；自动生成的 `.zom/` 按字母序混在
        // 大写目录前面（'.' < 'A'）。
        assert_eq!(
            names,
            vec![
                root_display_name(&root),
                ".zom".to_string(),
                "A_dir".to_string(),
                "b_dir".to_string(),
                "Afile".to_string(),
                "zfile".to_string(),
            ]
        );
    }

    /// 新项目根没有 `.zom/.zomignore` 时，构造期应自动建目录并写入默认模板。
    #[test]
    fn project_tree_new_should_create_default_zomignore_when_missing() {
        let root = tmp_root("zomignore-default");
        assert!(!root.join(".zom").exists());

        let _tree = ProjectTree::new(root.clone()).unwrap();

        let path = root.join(".zom").join(".zomignore");
        assert!(path.is_file(), "应自动创建 .zom/.zomignore");
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, ZOMIGNORE_DEFAULT);
    }

    /// 已存在 `.zom/.zomignore` 时不被覆盖——保护用户编辑。
    #[test]
    fn project_tree_new_should_not_overwrite_existing_zomignore() {
        let root = tmp_root("zomignore-preserve");
        create_dir_all(root.join(".zom")).unwrap();
        let path = root.join(".zom").join(".zomignore");
        std::fs::write(&path, "# my custom rules\nnotes.md\n").unwrap();

        let _tree = ProjectTree::new(root.clone()).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "# my custom rules\nnotes.md\n");
    }

    /// `.gitignore` 默认继承，里面列出的目录 / 文件应从文件树消失。
    #[test]
    fn project_tree_should_honor_root_gitignore() {
        let root = tmp_root("respect-gitignore");
        std::fs::write(root.join(".gitignore"), "target/\nsecret.txt\n").unwrap();
        create_dir_all(root.join("target/debug")).unwrap();
        File::create(root.join("target/debug/zom")).unwrap();
        File::create(root.join("secret.txt")).unwrap();
        File::create(root.join("README.md")).unwrap();
        // 让 `.zom/` 也对断言隐形，聚焦在 .gitignore 行为上。
        create_dir_all(root.join(".zom")).unwrap();
        std::fs::write(root.join(".zom/.zomignore"), ".zom/\n").unwrap();

        let tree = ProjectTree::new(root.clone()).unwrap();
        let names: Vec<String> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.name.to_string())
            .collect();
        // .gitignore 自身按 git 习惯不被忽略；target/ 与 secret.txt 不应出现。
        assert!(names.contains(&".gitignore".to_string()));
        assert!(names.contains(&"README.md".to_string()));
        assert!(!names.contains(&"target".to_string()));
        assert!(!names.contains(&"secret.txt".to_string()));
    }

    /// `.zomignore` 用 `!pattern` 反向放开 `.gitignore` 隐藏的项。
    #[test]
    fn zomignore_should_be_able_to_unignore_gitignored_paths() {
        let root = tmp_root("zomignore-unignore");
        std::fs::write(root.join(".gitignore"), "dist/\n").unwrap();
        create_dir_all(root.join("dist")).unwrap();
        File::create(root.join("dist/bundle.js")).unwrap();
        create_dir_all(root.join(".zom")).unwrap();
        // 同时压住 `.zom/` 自身，并放开 `dist/`。
        std::fs::write(root.join(".zom/.zomignore"), ".zom/\n!dist/\n").unwrap();

        let tree = ProjectTree::new(root.clone()).unwrap();
        let names: Vec<String> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.name.to_string())
            .collect();
        assert!(
            names.contains(&"dist".to_string()),
            "`!dist/` 应让目录回到文件树：{names:?}"
        );
    }

    /// 默认 `.zomignore` 模板**只有反向放开规则**，不主动隐藏任何东西——
    /// 没 `.gitignore` 的项目里，`.DS_Store`、`.git/` 等都该默认可见。
    #[test]
    fn default_zomignore_should_not_hide_anything_without_gitignore() {
        let root = tmp_root("default-no-rules");
        create_dir_all(root.join(".git/objects")).unwrap();
        File::create(root.join(".DS_Store")).unwrap();
        File::create(root.join("main.rs")).unwrap();
        File::create(root.join("notes.swp")).unwrap();

        let tree = ProjectTree::new(root.clone()).unwrap();
        let names: Vec<String> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.name.to_string())
            .collect();
        assert!(names.contains(&".git".to_string()), "{names:?}");
        assert!(names.contains(&".DS_Store".to_string()), "{names:?}");
        assert!(names.contains(&"notes.swp".to_string()), "{names:?}");
        assert!(names.contains(&"main.rs".to_string()), "{names:?}");
    }

    /// 默认模板里的 `!.env.example` 等放开规则：当 `.gitignore` 用通配把它们
    /// 隐藏时，模板应把它们取回。把「默认有效」钉成契约——以后增删默认放开项
    /// 至少有一个 case 在测一头。
    #[test]
    fn default_zomignore_should_unhide_common_examples_and_keepers() {
        let root = tmp_root("default-unhide");
        std::fs::write(
            root.join(".gitignore"),
            ".env*\n*.keep\n*.gitkeep\ntarget/\ndist/\nbuild/\nout/\n",
        )
        .unwrap();
        File::create(root.join(".env")).unwrap();
        File::create(root.join(".env.local")).unwrap();
        File::create(root.join(".env.example")).unwrap();
        File::create(root.join(".env.sample")).unwrap();
        File::create(root.join(".env.local.example")).unwrap();
        create_dir_all(root.join("empty_dir")).unwrap();
        File::create(root.join("empty_dir/.gitkeep")).unwrap();
        File::create(root.join("empty_dir/.keep")).unwrap();
        create_dir_all(root.join("target/debug")).unwrap();
        create_dir_all(root.join("dist")).unwrap();
        create_dir_all(root.join("build")).unwrap();
        create_dir_all(root.join("out")).unwrap();

        let mut tree = ProjectTree::new(root.clone()).unwrap();
        let names: Vec<String> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.name.to_string())
            .collect();
        // 默认 !.env / !.env.* 把 .env 本体和示例放回来。
        assert!(names.contains(&".env".to_string()), "{names:?}");
        // .env.local 也被 !.env.* 取回。
        assert!(names.contains(&".env.local".to_string()), "{names:?}");
        assert!(names.contains(&".env.example".to_string()), "{names:?}");
        assert!(names.contains(&".env.sample".to_string()), "{names:?}");
        assert!(
            names.contains(&".env.local.example".to_string()),
            "{names:?}"
        );
        // 默认 !target/ / !dist/ / !build/ / !out/ 把构建产物放回来。
        assert!(names.contains(&"target".to_string()), "{names:?}");
        assert!(names.contains(&"dist".to_string()), "{names:?}");
        assert!(names.contains(&"build".to_string()), "{names:?}");
        assert!(names.contains(&"out".to_string()), "{names:?}");

        // 占位文件——展开后该可见。
        tree.expand(&root.join("empty_dir")).unwrap();
        let names: Vec<String> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.name.to_string())
            .collect();
        assert!(names.contains(&".gitkeep".to_string()), "{names:?}");
        assert!(names.contains(&".keep".to_string()), "{names:?}");
    }
}
