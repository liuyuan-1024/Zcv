//! Git 状态查询服务。
//!
//! 本模块负责：
//! - 探测 git 仓库
//! - 解析 `git status --porcelain=v1` 输出
//! - 缓存文件级 git 状态供文件树和 editor gutter 查询
//!
//! 设计对齐 Zed：完整保留 XY 双字符语义（暂存区 + 工作区各自独立状态），文件树着色时归约为简化 ColorKind（不区分 staged/unstaged）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 单文件 git 状态，完整保留 `--porcelain=v1` 的 XY 双字符语义。
///
/// `X` = 暂存区（index）状态，`Y` = 工作区（worktree）状态。
/// 文件树着色不区分这两维，但保留完整信息供后续 Git Panel 使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitStatus {
    /// 已跟踪文件：暂存区和工作区各自独立的状态。
    /// 例如 `M ` = 仅暂存区修改，` M` = 仅工作区修改，`MM` = 两区都修改。
    Tracked {
        index: StatusCode,
        worktree: StatusCode,
    },
    /// 未合并（冲突），对应 `UU`、`AA`、`DD`、`AU`、`UA`、`DU`、`UD`。
    Unmerged,
    /// 未跟踪，对应 `??`。
    Untracked,
    /// 被 .gitignore 忽略，对应 `!!`。
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCode {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    TypeChanged,
}

/// 文件树着色用的简化状态。
///
/// 文件树不需要区分 staged/unstaged —— "这个文件有没有待处理的变化"就够了。
/// 细粒度的暂存/未暂存区分留给 Git Panel。
///
/// 变体按严重程度升序排列（`#[derive(Ord)]` 依赖此顺序）：
/// Ignored < Untracked < Added < Modified < Deleted < Conflict。
/// 目录聚合时取 [`std::cmp::max`] ——子文件最严重的状态冒泡到父目录。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ColorKind {
    Ignored,
    Untracked,
    Added,
    Modified,
    Deleted,
    Conflict,
}

impl GitStatus {
    /// 归约为文件树着色用的颜色类别。
    ///
    /// 优先取工作区状态；工作区无变更时看暂存区。
    /// Unmodified 时返回 None（不染色）。
    pub fn color_kind(&self) -> Option<ColorKind> {
        match self {
            GitStatus::Tracked { index, worktree } => {
                // 工作区有变化优先用工作区，否则看暂存区
                let decisive = if *worktree != StatusCode::Unmodified {
                    worktree
                } else {
                    index
                };
                match decisive {
                    StatusCode::Modified | StatusCode::Renamed | StatusCode::TypeChanged => {
                        Some(ColorKind::Modified)
                    }
                    StatusCode::Added => Some(ColorKind::Added),
                    StatusCode::Deleted => Some(ColorKind::Deleted),
                    StatusCode::Unmodified => None,
                }
            }
            GitStatus::Unmerged => Some(ColorKind::Conflict),
            GitStatus::Untracked => Some(ColorKind::Untracked),
            GitStatus::Ignored => Some(ColorKind::Ignored),
        }
    }
}

pub struct GitService {
    /// git 仓库根目录（通过 `git rev-parse --show-toplevel` 获得）。
    /// 可能与项目根不同（用户可能打开了仓库的子目录）。
    repo_root: PathBuf,
    /// 文件级状态缓存：相对路径（相对于 repo_root）→ 状态。
    statuses: HashMap<PathBuf, GitStatus>,
    /// 目录级聚合颜色：子文件状态向上冒泡到祖先目录。
    /// refresh 时自动计算。
    dir_colors: HashMap<PathBuf, ColorKind>,
    /// 当前目录是否在一个有效的 git 仓库内。
    valid: bool,
    /// 刷新代际：每次 [`refresh`](Self::refresh) 成功完成后递增。
    /// 消费方（如 VersionControlModel）比对代际号来判断缓存是否失效。
    generation: u64,
}

impl GitService {
    /// 以项目根目录为起点探测 git 仓库。
    ///
    /// 即使项目不在 git 仓库内也会成功构造（`valid = false`），后续查询和刷新都是空操作。
    pub fn new(project_root: &Path) -> Self {
        let (valid, repo_root) = match Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(project_root)
            .output()
        {
            Ok(output) if output.status.success() => {
                let root_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                // canonicalize 消解 /tmp → /private/tmp 等符号链接，保证后续 strip_prefix 正确
                let root = PathBuf::from(root_str);
                let canonical = root.canonicalize().unwrap_or(root);
                (true, canonical)
            }
            _ => (false, project_root.to_path_buf()),
        };

        Self {
            repo_root,
            statuses: HashMap::new(),
            dir_colors: HashMap::new(),
            valid,
            generation: 0,
        }
    }

    pub fn is_git_repo(&self) -> bool {
        self.valid
    }

    /// 仓库根目录绝对路径。
    pub fn repo_root_path(&self) -> &Path {
        &self.repo_root
    }

    /// 刷新代际号。消费方通过比对代际号判断是否需要重建缓存。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 仓库根目录的展示名（目录名）。
    pub fn root_name(&self) -> String {
        self.repo_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.repo_root.to_string_lossy().into_owned())
    }

    /// 返回所有文件级 git 状态的只读引用。
    /// key 为相对于 repo_root 的路径。
    pub fn statuses(&self) -> &HashMap<PathBuf, GitStatus> {
        &self.statuses
    }

    /// 返回目录级聚合颜色的只读引用。
    /// key 为相对于 repo_root 的路径。
    pub fn dir_colors(&self) -> &HashMap<PathBuf, ColorKind> {
        &self.dir_colors
    }

    /// 在内存中翻转暂存状态（乐观更新，不调 git）。
    ///
    /// 用于点击复选框后立即更新 UI，真实 git 操作稍后执行。
    pub fn flip_staged_in_memory(&mut self, rel_path: &Path, staged: bool) {
        match self.statuses.get(rel_path) {
            Some(GitStatus::Tracked { index, worktree }) => {
                let (new_index, new_worktree) = if staged {
                    // 暂存：把 worktree 状态移到 index
                    (*worktree, StatusCode::Unmodified)
                } else {
                    // 取消暂存：把 index 状态移回 worktree
                    let new_worktree = match index {
                        StatusCode::Deleted => StatusCode::Deleted,
                        _ => StatusCode::Modified,
                    };
                    (StatusCode::Unmodified, new_worktree)
                };
                self.statuses.insert(
                    rel_path.to_path_buf(),
                    GitStatus::Tracked {
                        index: new_index,
                        worktree: new_worktree,
                    },
                );
            }
            Some(GitStatus::Untracked) if staged => {
                self.statuses.insert(
                    rel_path.to_path_buf(),
                    GitStatus::Tracked {
                        index: StatusCode::Added,
                        worktree: StatusCode::Unmodified,
                    },
                );
            }
            Some(GitStatus::Unmerged) if staged => {
                self.statuses.insert(
                    rel_path.to_path_buf(),
                    GitStatus::Tracked {
                        index: StatusCode::Modified,
                        worktree: StatusCode::Unmodified,
                    },
                );
            }
            _ => return,
        }
        self.reindex_dir_colors();
    }

    /// `git add`、`git reset` 后刷新单个文件的 git 状态。
    ///
    /// 只运行 `git status --porcelain=v1 -- <path>`，速度快于全仓库扫描。
    /// 之后重建 dir_colors（纯内存操作）。
    pub fn refresh_single(&mut self, rel_path: &Path) -> Result<(), String> {
        if !self.valid {
            return Ok(());
        }
        let output = Command::new("git")
            .args(["status", "--porcelain=v1", "--"])
            .arg(rel_path)
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| format!("无法执行 git status：{e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        // 解析单文件输出："XY path" 或空（文件干净，无变更）
        if let Some(line) = stdout.lines().next() {
            if line.len() >= 4 {
                let bytes = line.as_bytes();
                let x = bytes[0];
                let y = bytes[1];
                let path_str = line[3..].trim();
                // 处理重命名："R  old -> new" 取新路径
                let path_str = if x == b'R' || y == b'R' {
                    path_str.rsplit(" -> ").next().unwrap_or(path_str)
                } else {
                    path_str
                };
                let status = classify_xy(x, y);
                self.statuses.insert(PathBuf::from(path_str), status);
            }
        } else {
            // 文件已干净 —— 从 statuses 中移除。
            self.statuses.remove(rel_path);
        }

        self.reindex_dir_colors();
        self.generation += 1;
        Ok(())
    }

    /// 从 statuses 重建 dir_colors。纯内存操作，不调 git。
    fn reindex_dir_colors(&mut self) {
        let mut dir_colors: HashMap<PathBuf, ColorKind> = HashMap::new();
        for (rel_path, status) in &self.statuses {
            let Some(color) = status.color_kind() else {
                continue;
            };
            dir_colors
                .entry(rel_path.clone())
                .and_modify(|e| *e = std::cmp::max(*e, color))
                .or_insert(color);
            if color == ColorKind::Ignored {
                continue;
            }
            let mut parent = rel_path.parent();
            while let Some(p) = parent {
                if p.as_os_str().is_empty() {
                    break;
                }
                let entry = dir_colors.entry(p.to_path_buf()).or_insert(color);
                *entry = std::cmp::max(*entry, color);
                parent = p.parent();
            }
        }
        self.dir_colors = dir_colors;
    }

    /// 刷新 git 状态缓存。
    ///
    /// 非 git 仓库时直接返回 Ok。
    pub fn refresh(&mut self) -> Result<(), String> {
        if !self.valid {
            return Ok(());
        }

        // 1. tracked / untracked / modified / deleted：`git status -u`。
        //    不加 --ignored——含 --ignored 会递归列出 target/ 深处所有文件
        //   （本项目 >80 万行），耗时数秒。
        let output = Command::new("git")
            .args(["status", "--porcelain=v1", "-u"])
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| format!("无法执行 git：{e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(stderr.trim().to_string());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut new_statuses = HashMap::new();

        for line in stdout.lines() {
            if line.len() < 4 {
                continue;
            }
            let bytes = line.as_bytes();
            let x = bytes[0]; // 暂存区状态
            let y = bytes[1]; // 工作区状态

            // 路径从第 3 个字符开始："XY " 之后
            let path_part = &line[3..];

            // 处理重命名/复制：格式为 "R  old_path -> new_path"
            let path_str = if x == b'R' || x == b'C' || y == b'R' || y == b'C' {
                // 取 -> 后面的新路径
                path_part.rsplit(" -> ").next().unwrap_or(path_part)
            } else {
                path_part
            };

            // 去掉路径首尾的空白和引号
            let path_str = path_str.trim();
            // git 会对含非 ASCII 字符的路径做 C 风格引用：外层双引号 + 八进制转义。
            // 例如 "\346\236\266\346\236\204.md" 实际对应 "架构.md"。
            let path_str = if path_str.starts_with('"') && path_str.ends_with('"') {
                unquote_git_path(&path_str[1..path_str.len() - 1])
            } else {
                path_str.to_string()
            };

            let status = classify_xy(x, y);
            new_statuses.insert(PathBuf::from(&path_str), status);
        }

        // 2. Ignored 文件/目录：`git ls-files --others --ignored --exclude-standard --directory`。
        //    --directory 让 git 只输出被忽略目录的顶层条目（如 `target/`），不递归内部文件，
        //    避免了 previous 方案里 `git status --ignored` 递归 80 万行的问题。
        if let Ok(output) = Command::new("git")
            .args([
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "-z",
            ])
            .current_dir(&self.repo_root)
            .output()
        {
            if output.status.success() {
                for chunk in output.stdout.split(|&b| b == 0) {
                    if chunk.is_empty() {
                        continue;
                    }
                    if let Ok(s) = std::str::from_utf8(chunk) {
                        // 去掉可能存在的末尾 `/`（目录条目）
                        let path = s.trim_end_matches('/');
                        new_statuses
                            .entry(PathBuf::from(path))
                            .or_insert(GitStatus::Ignored);
                    }
                }
            }
        }

        self.statuses = new_statuses;
        self.reindex_dir_colors();
        self.generation += 1;
        Ok(())
    }

    /// 查询单个文件（绝对路径）的 git 状态。
    ///
    /// `abs_path` 必须是 repo_root 下的文件，否则返回 None。
    pub fn file_status(&self, abs_path: &Path) -> Option<&GitStatus> {
        let rel = self.rel_path(abs_path)?;
        self.statuses.get(&rel)
    }

    /// 查询单个文件的颜色类别。
    ///
    /// 文件树着色入口：有变更返回对应 ColorKind，无变更返回 None。
    pub fn color_kind(&self, abs_path: &Path) -> Option<ColorKind> {
        self.file_status(abs_path).and_then(|s| s.color_kind())
    }

    /// 查询目录的聚合颜色类别。
    ///
    /// 目录颜色是其所有后代文件中最严重的 git 状态——子文件状态向上冒泡，折叠目录时也能看到里面有变更。
    pub fn directory_color_kind(&self, abs_path: &Path) -> Option<ColorKind> {
        let rel = self.rel_path(abs_path)?;
        self.dir_colors.get(&rel).copied()
    }

    /// 把绝对路径转为相对于 repo_root 的相对路径。
    ///
    /// FileTreeModel 在 open_project 入口 canonicalize 项目根，因此绝大多数查询直接走快速路径（零 syscall）。
    /// 仅符号链接等边缘情况会触发父目录 canonicalize。
    fn rel_path(&self, abs_path: &Path) -> Option<PathBuf> {
        // 快速路径：两边都是 canonical，直接 strip
        if let Ok(rel) = abs_path.strip_prefix(&self.repo_root) {
            return Some(rel.to_path_buf());
        }
        // 慢速路径：符号链接导致前缀不匹配。只 canonicalize 父目录——
        // 文件可能已被删除（git status 中的 D），整路径 canonicalize 会失败。
        let parent = abs_path.parent()?.canonicalize().ok()?;
        let resolved = parent.join(abs_path.file_name()?);
        let rel = resolved.strip_prefix(&self.repo_root).ok()?;
        Some(rel.to_path_buf())
    }

    /// 查询单个文件的 diff hunk 列表（相对 HEAD）。
    ///
    /// 解析 `git diff -U0 HEAD -- <file>` 的 hunk header，返回工作区行范围。
    /// 非 git 仓库或无变更时返回空 Vec。
    pub fn diff_hunks(&self, abs_path: &Path) -> Vec<DiffHunk> {
        if !self.valid {
            return Vec::new();
        }
        let output = match Command::new("git")
            .args(["diff", "-U0", "HEAD", "--"])
            .arg(abs_path)
            .current_dir(&self.repo_root)
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_diff_hunks(&stdout)
    }

    /// `git add <path>` —— 将文件变更加入暂存区。
    ///
    /// `rel_path` 为相对于 `repo_root` 的路径。
    pub fn stage_file(&self, rel_path: &Path) -> Result<(), String> {
        if !self.valid {
            return Err("不在 Git 仓库中".to_string());
        }
        let output = Command::new("git")
            .args(["add", "--"])
            .arg(rel_path)
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| format!("无法执行 git add：{e}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(())
    }

    /// `git reset HEAD <path>` —— 将文件从暂存区移除。
    ///
    /// `rel_path` 为相对于 `repo_root` 的路径。
    pub fn unstage_file(&self, rel_path: &Path) -> Result<(), String> {
        if !self.valid {
            return Err("不在 Git 仓库中".to_string());
        }
        let output = Command::new("git")
            .args(["reset", "HEAD", "--"])
            .arg(rel_path)
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| format!("无法执行 git reset：{e}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(())
    }

    /// 执行 git commit。
    ///
    /// 提交信息可以包含换行符，git 会正确解析多行 commit message。
    pub fn commit(&self, message: &str) -> Result<(), String> {
        if !self.valid {
            return Err("不在 Git 仓库中".to_string());
        }
        let output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| format!("无法执行 git commit：{e}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(())
    }

    /// 暂存所有变更：`git add -A`。
    pub fn stage_all(&self) -> Result<(), String> {
        self.run_git(&["add", "-A"])
    }

    /// 取消暂存所有文件：`git reset HEAD`。
    pub fn unstage_all(&self) -> Result<(), String> {
        self.run_git(&["reset", "HEAD"])
    }

    /// 从远端拉取：`git fetch`。
    // 预留给后续 Git Panel。
    #[allow(dead_code)]
    pub fn fetch(&self) -> Result<(), String> {
        self.run_git(&["fetch"])
    }

    /// 拉取并合并：`git pull`。
    // 预留给后续 Git Panel。
    #[allow(dead_code)]
    pub fn pull(&self) -> Result<(), String> {
        self.run_git(&["pull"])
    }

    /// 变更统计：(增行数, 删行数)，不受暂存状态影响。
    ///
    /// `git diff --numstat HEAD` 一口统计暂存区 + 工作区相对 HEAD 的总变更。
    /// 未跟踪文件单独统计行数（视为纯新增）。
    pub fn diff_stats(&self) -> (u32, u32) {
        let mut added: u32 = 0;
        let mut deleted: u32 = 0;
        // 暂存区 + 工作区相对 HEAD 的总变更
        if let Ok(output) = Command::new("git")
            .args(["diff", "--numstat", "HEAD"])
            .current_dir(&self.repo_root)
            .output()
        {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 2 {
                    added += parts[0].parse::<u32>().unwrap_or(0);
                    deleted += parts[1].parse::<u32>().unwrap_or(0);
                }
            }
        }
        // 未跟踪文件：不在 diff 中，按文件行数计入新增。
        // 用 BufRead 增量读取，避免大文件全部加载到内存。
        let untracked: Vec<PathBuf> = self
            .statuses
            .iter()
            .filter(|(_, s)| matches!(s, GitStatus::Untracked))
            .map(|(p, _)| self.repo_root.join(p))
            .collect();
        for path in &untracked {
            if let Ok(file) = std::fs::File::open(path) {
                use std::io::BufRead;
                added += std::io::BufReader::new(file).lines().count() as u32;
            }
        }
        (added, deleted)
    }

    fn run_git(&self, args: &[&str]) -> Result<(), String> {
        if !self.valid {
            return Err("不在 Git 仓库中".to_string());
        }
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| format!("无法执行 git：{e}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(())
    }
}

/// 一个 diff hunk 的行范围描述——gutter 据此画色条。
///
/// `new_start` 和 `new_lines` 描述工作区文件中的行范围（1-based）。
#[derive(Debug, Clone, Copy)]
pub struct DiffHunk {
    pub new_start: u32,
    pub new_lines: u32,
    pub kind: DiffHunkKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffHunkKind {
    /// 新增行（只出现在工作区）。
    Added,
    /// 修改行（HEAD 和工作区都有，内容不同）。
    Modified,
    /// 删除行（只出现在 HEAD）。gutter 不直接对应到工作区行，用 old_start 标记位置。
    Deleted,
}

/// 解析 unified diff 输出的 hunk header：`@@ -old_start,old_lines +new_start,new_lines @@`
///
/// 用 `@@` 定界取数字段，不依赖 `+`/`-` 不出现在路径或函数名中的假设。
fn parse_diff_hunks(diff: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    for line in diff.lines() {
        // 格式：@@ -old_start,old_count +new_start,new_count @@ 可选尾部上下文
        // 用 "@@" 劈开取中间的数字段，不依赖 '+'/'-' 不出现在路径中。
        let body = match line.split("@@").nth(1) {
            Some(s) => s.trim(),
            None => continue,
        };
        // body: "-10,3 +10,5"
        let mut parts = body.split_whitespace();
        let (Some(minus), Some(plus)) = (parts.next(), parts.next()) else {
            continue;
        };
        let parse_pair = |s: &str| -> (u32, u32) {
            let mut ns = s[1..].split(',');
            let start: u32 = ns.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            let count: u32 = ns.next().and_then(|n| n.parse().ok()).unwrap_or(1);
            (start, count)
        };
        let (new_start, new_lines) = parse_pair(plus);
        let (_old_start, old_lines) = parse_pair(minus);

        let kind = if new_lines == 0 {
            DiffHunkKind::Deleted
        } else if old_lines == 0 {
            DiffHunkKind::Added
        } else {
            DiffHunkKind::Modified
        };
        hunks.push(DiffHunk {
            new_start,
            new_lines,
            kind,
        });
    }
    hunks
}

/// 把 XY 双字符映射为 GitStatus。
fn classify_xy(x: u8, y: u8) -> GitStatus {
    match (x, y) {
        (b'?', b'?') => GitStatus::Untracked,
        (b'!', b'!') => GitStatus::Ignored,
        // 未合并：至少一方是 U，或者双方都是 A/D（add/add、del/del 冲突）
        (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => GitStatus::Unmerged,
        _ => {
            let index = parse_code(x);
            let worktree = parse_code(y);
            GitStatus::Tracked { index, worktree }
        }
    }
}

fn parse_code(c: u8) -> StatusCode {
    match c {
        b'M' => StatusCode::Modified,
        b'A' => StatusCode::Added,
        b'D' => StatusCode::Deleted,
        b'R' => StatusCode::Renamed,
        b'C' => StatusCode::Modified, // 复制视为修改
        b'T' => StatusCode::TypeChanged,
        _ => StatusCode::Unmodified,
    }
}

/// 解码 git 对含非 ASCII 字符路径的 C 风格八进制转义。
///
/// git 的 `core.quotePath` 默认为 true，会将路径中非 ASCII 字节写成 `\ooo` 格式
/// （`\` 后跟 3 位八进制数）。例如 `\346\236\266\346\236\204.md` 解码后为 `架构.md`。
///
/// 此外也处理 `\\`、`\"`、`\n`、`\t` 等标准 C 转义。
fn unquote_git_path(quoted: &str) -> String {
    let mut bytes = Vec::with_capacity(quoted.len());
    let mut chars = quoted.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            // 直接推入 UTF-8 字节（ASCII 路径的常见情况）
            bytes.extend_from_slice(ch.encode_utf8(&mut [0u8; 4]).as_bytes());
            continue;
        }
        // 处理转义序列
        match chars.next() {
            Some('\\') => bytes.push(b'\\'),
            Some('"') => bytes.push(b'"'),
            Some('n') => bytes.push(b'\n'),
            Some('t') => bytes.push(b'\t'),
            Some(d0) if d0.is_ascii_digit() => {
                // 八进制转义：\ooo（最多 3 位）
                let v0 = (d0 as u8) - b'0';
                let mut val: u16 = v0 as u16;
                // 再取最多两个八进制数字
                if let Some(&d1) = chars.peek()
                    && d1.is_ascii_digit()
                {
                    chars.next();
                    val = val * 8 + (d1 as u8 - b'0') as u16;
                }
                if let Some(&d2) = chars.peek()
                    && d2.is_ascii_digit()
                {
                    chars.next();
                    val = val * 8 + (d2 as u8 - b'0') as u16;
                }
                bytes.push(val as u8);
            }
            // 不认识的转义序列：保持原样（包括 `\` 和后面的字符）
            Some(other) => {
                bytes.push(b'\\');
                bytes.extend_from_slice(other.encode_utf8(&mut [0u8; 4]).as_bytes());
            }
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::process::Command as StdCommand;

    /// 在临时目录初始化一个 git 仓库，返回仓库根路径。
    fn init_git_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zom-git-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        StdCommand::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&dir)
            .output()
            .unwrap();
        // 配置 git user，否则 commit 会失败
        StdCommand::new("git")
            .args(["config", "user.email", "test@zom.local"])
            .current_dir(&dir)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.name", "zom-test"])
            .current_dir(&dir)
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn non_git_project_should_be_invalid() {
        let dir = std::env::temp_dir();
        let mut service = GitService::new(&dir);
        assert!(!service.is_git_repo());
        // refresh 在 non-git 项目上不应报错
        assert!(service.refresh().is_ok());
    }

    #[test]
    fn untracked_file_should_be_detected() {
        let repo = init_git_repo("untracked");
        File::create(repo.join("new.txt")).unwrap();

        let mut service = GitService::new(&repo);
        assert!(service.is_git_repo());
        service.refresh().unwrap();

        let status = service.file_status(&repo.join("new.txt")).unwrap();
        assert_eq!(*status, GitStatus::Untracked);
        assert_eq!(status.color_kind(), Some(ColorKind::Untracked));
    }

    #[test]
    fn modified_file_should_be_detected() {
        let repo = init_git_repo("modified");
        let path = repo.join("mod.txt");
        File::create(&path).unwrap();
        // git add + commit，再修改
        StdCommand::new("git")
            .args(["add", "mod.txt"])
            .current_dir(&repo)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&repo)
            .output()
            .unwrap();
        fs::write(&path, b"changed").unwrap();

        let mut service = GitService::new(&repo);
        service.refresh().unwrap();

        let status = service.file_status(&path).unwrap();
        assert_eq!(
            *status,
            GitStatus::Tracked {
                index: StatusCode::Unmodified,
                worktree: StatusCode::Modified,
            }
        );
        assert_eq!(status.color_kind(), Some(ColorKind::Modified));
    }

    #[test]
    fn staged_file_should_be_detected() {
        let repo = init_git_repo("staged");
        let path = repo.join("staged.txt");
        File::create(&path).unwrap();
        StdCommand::new("git")
            .args(["add", "staged.txt"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let mut service = GitService::new(&repo);
        service.refresh().unwrap();

        let status = service.file_status(&path).unwrap();
        assert_eq!(
            *status,
            GitStatus::Tracked {
                index: StatusCode::Added,
                worktree: StatusCode::Unmodified,
            }
        );
        // 暂存区 Added → 文件树着色 = Added（绿色）
        assert_eq!(status.color_kind(), Some(ColorKind::Added));
    }

    #[test]
    fn ignored_file_with_ignored_flag() {
        // --ignored 让 git status 输出被忽略文件，用于文件树着色区分。
        let repo = init_git_repo("ignored");
        fs::write(repo.join(".gitignore"), "*.ignored\n").unwrap();
        File::create(repo.join("test.ignored")).unwrap();

        let mut service = GitService::new(&repo);
        service.refresh().unwrap();

        // 带 --ignored 时被忽略文件应返回 Ignored 颜色
        let kind = service.color_kind(&repo.join("test.ignored"));
        assert_eq!(kind, Some(ColorKind::Ignored));
    }

    #[test]
    fn deleted_file_should_be_detected() {
        let repo = init_git_repo("deleted");
        let path = repo.join("del.txt");
        File::create(&path).unwrap();
        StdCommand::new("git")
            .args(["add", "del.txt"])
            .current_dir(&repo)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&repo)
            .output()
            .unwrap();
        fs::remove_file(&path).unwrap();

        let mut service = GitService::new(&repo);
        service.refresh().unwrap();

        let status = service.file_status(&path).unwrap();
        assert_eq!(
            *status,
            GitStatus::Tracked {
                index: StatusCode::Unmodified,
                worktree: StatusCode::Deleted,
            }
        );
        assert_eq!(status.color_kind(), Some(ColorKind::Deleted));
    }

    #[test]
    fn clean_file_should_return_none_color() {
        let repo = init_git_repo("clean");
        let path = repo.join("clean.txt");
        File::create(&path).unwrap();
        StdCommand::new("git")
            .args(["add", "clean.txt"])
            .current_dir(&repo)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let mut service = GitService::new(&repo);
        service.refresh().unwrap();

        // 干净文件不在 status 输出里
        assert!(service.file_status(&path).is_none());
        assert_eq!(service.color_kind(&path), None);
    }

    #[test]
    fn subdirectory_project_should_still_find_repo_root() {
        let repo = init_git_repo("subdir-project");
        let subdir = repo.join("subdir");
        fs::create_dir_all(&subdir).unwrap();
        File::create(repo.join("root_file.txt")).unwrap();

        // 以子目录为项目根
        let mut service = GitService::new(&subdir);
        assert!(service.is_git_repo());
        service.refresh().unwrap();

        // root_file.txt 的路径应该相对于 repo_root 查询
        let status = service.file_status(&repo.join("root_file.txt")).unwrap();
        assert_eq!(*status, GitStatus::Untracked);
    }

    #[test]
    fn parse_diff_hunks_should_extract_added_modified_deleted() {
        let diff = "\
diff --git a/src/lib.rs b/src/lib.rs
index 123..456 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -10,3 +10,5 @@ fn main() {
-    let x = 1;
+    let x = 2;
+    let y = 3;
@@ -20,0 +25,3 @@ fn foo() {
+    let z = 4;
@@ -30,3 +35,0 @@ fn bar() {
-    let w = 5;
";
        let hunks = parse_diff_hunks(diff);
        assert_eq!(hunks.len(), 3);

        // @@ -10,3 +10,5 @@ → old=3, new=5 → Modified
        assert_eq!(hunks[0].new_start, 10);
        assert_eq!(hunks[0].new_lines, 5);
        assert!(matches!(hunks[0].kind, DiffHunkKind::Modified));

        // @@ -20,0 +25,3 @@ → old=0, new=3 → Added
        assert_eq!(hunks[1].new_start, 25);
        assert_eq!(hunks[1].new_lines, 3);
        assert!(matches!(hunks[1].kind, DiffHunkKind::Added));

        // @@ -30,3 +35,0 @@ → old=3, new=0 → Deleted
        assert_eq!(hunks[2].new_start, 35);
        assert_eq!(hunks[2].new_lines, 0);
        assert!(matches!(hunks[2].kind, DiffHunkKind::Deleted));
    }

    #[test]
    fn unquote_git_path_should_decode_octal_escapes() {
        // 中文文件名 "架构原则.md" 的八进制转义形式
        assert_eq!(
            unquote_git_path(r"\346\236\266\346\236\204\345\216\237\345\210\231.md"),
            "架构原则.md"
        );
    }

    #[test]
    fn unquote_git_path_should_handle_mixed_ascii_and_escape() {
        assert_eq!(
            unquote_git_path(r"src/\346\265\213\350\257\225/mod.rs"),
            "src/测试/mod.rs"
        );
    }

    #[test]
    fn unquote_git_path_should_preserve_plain_ascii() {
        assert_eq!(unquote_git_path("README.md"), "README.md");
        assert_eq!(unquote_git_path("src/lib.rs"), "src/lib.rs");
    }

    #[test]
    fn unquote_git_path_should_handle_backslash_and_quote_escapes() {
        // \" → " 和 \\ → \
        assert_eq!(unquote_git_path("file\\\"name.txt"), "file\"name.txt");
        assert_eq!(
            unquote_git_path("path\\\\to\\\\file.txt"),
            "path\\to\\file.txt"
        );
    }
}
