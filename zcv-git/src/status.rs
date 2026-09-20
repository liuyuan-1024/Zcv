//! git 输出解析：`git status --porcelain=v1` 与 `git diff --numstat`。

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context as _, Result};

/// 单项的索引（index）/工作区（worktree）状态码，对应 porcelain 输出中的单个字符。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusCode {
    #[default]
    Unmodified,
    Modified,
    TypeChanged,
    Added,
    Deleted,
}

impl StatusCode {
    /// 解析 `--no-renames` porcelain 状态字符。
    fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            b'M' => Ok(StatusCode::Modified),
            b'T' => Ok(StatusCode::TypeChanged),
            b'A' => Ok(StatusCode::Added),
            b'D' => Ok(StatusCode::Deleted),
            b' ' => Ok(StatusCode::Unmodified),
            _ => anyhow::bail!("无效的 git 状态码：{byte}"),
        }
    }
}

/// 文件的完整 git 状态：索引 × 工作区 二维，外加未跟踪/忽略/冲突特殊态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileStatus {
    #[default]
    Untracked,
    Ignored,
    Unmerged,
    Tracked {
        index_status: StatusCode,
        worktree_status: StatusCode,
    },
}

impl FileStatus {
    /// 从 porcelain 输出的两位状态码生成 FileStatus。
    ///
    /// 参考 https://git-scm.com/docs/git-status#_output
    /// 注意：git 输出里"无变化"是空白字符，这里按惯例用空格 ` ` 表示。
    fn from_bytes(bytes: [u8; 2]) -> Result<Self> {
        let status = match bytes {
            [b'?', b'?'] => FileStatus::Untracked,
            [b'!', b'!'] => FileStatus::Ignored,
            // 冲突的所有组合（AA/DD/UU/AU/UA/DU/UD）统一记为 Unmerged。
            [b'A', b'A']
            | [b'D', b'D']
            | [b'U', b'U']
            | [b'A', b'U']
            | [b'U', b'A']
            | [b'D', b'U']
            | [b'U', b'D'] => FileStatus::Unmerged,
            [x, y] => FileStatus::Tracked {
                index_status: StatusCode::from_byte(x)?,
                worktree_status: StatusCode::from_byte(y)?,
            },
        };
        Ok(status)
    }

    pub fn is_modified(self) -> bool {
        match self {
            FileStatus::Tracked {
                index_status,
                worktree_status,
            } => {
                matches!(index_status, StatusCode::Modified)
                    || matches!(worktree_status, StatusCode::Modified)
            }
            _ => false,
        }
    }

    pub fn is_created(self) -> bool {
        match self {
            FileStatus::Tracked {
                index_status,
                worktree_status,
            } => {
                matches!(index_status, StatusCode::Added)
                    || matches!(worktree_status, StatusCode::Added)
            }
            FileStatus::Untracked => true,
            _ => false,
        }
    }

    pub fn is_deleted(self) -> bool {
        match self {
            FileStatus::Tracked {
                index_status,
                worktree_status,
            } => {
                matches!(index_status, StatusCode::Deleted)
                    || matches!(worktree_status, StatusCode::Deleted)
            }
            _ => false,
        }
    }

    pub fn is_untracked(self) -> bool {
        matches!(self, FileStatus::Untracked)
    }

    pub fn is_ignored(self) -> bool {
        matches!(self, FileStatus::Ignored)
    }

    /// 是否有已暂存的变更（index 相对 HEAD 有差异）。
    ///
    /// 面板目录暂存时按此过滤展开的文件集合；冲突条目恒为 false（须先解决冲突）。
    pub fn has_staged(self) -> bool {
        matches!(
            self,
            FileStatus::Tracked { index_status, .. } if index_status != StatusCode::Unmodified
        )
    }

    /// 是否有未暂存的变更（工作区相对 index 有差异，含未跟踪文件）。
    ///
    /// 冲突条目恒为 false（不参与暂存/取消暂存）。
    pub fn has_unstaged(self) -> bool {
        matches!(self, FileStatus::Untracked)
            || matches!(
                self,
                FileStatus::Tracked { worktree_status, .. }
                    if worktree_status != StatusCode::Unmodified
            )
    }

    /// 目录聚合优先级：conflict > deleted > modified > added/untracked > ignored > 无状态。
    ///
    /// 目录聚合时取子项中优先级最高的状态。
    pub fn priority(self) -> u8 {
        match self {
            FileStatus::Unmerged => 5,
            FileStatus::Tracked {
                index_status,
                worktree_status,
            } => {
                let deleted = matches!(index_status, StatusCode::Deleted)
                    || matches!(worktree_status, StatusCode::Deleted);
                let modified =
                    matches!(index_status, StatusCode::Modified | StatusCode::TypeChanged)
                        || matches!(
                            worktree_status,
                            StatusCode::Modified | StatusCode::TypeChanged
                        );
                let added = matches!(index_status, StatusCode::Added)
                    || matches!(worktree_status, StatusCode::Added);
                if deleted {
                    4
                } else if modified {
                    3
                } else if added {
                    2
                } else {
                    0
                }
            }
            FileStatus::Untracked => 2,
            FileStatus::Ignored => 1,
        }
    }
}

/// 分支头行信息（`git status --porcelain=v1 -b` 的第一条记录）。
///
/// 形如 `## <branch>[...<upstream>[ [ahead N, behind M]|[gone]]]`；
/// 无 upstream 时 `...` 段与方括号段都不存在（含 detached HEAD、空仓库形态）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchStatus {
    /// 当前分支名（短名）；detached HEAD 与空仓库（无提交）时为 None。
    pub branch: Option<String>,
    /// upstream 跟踪名（如 `origin/main`）；无 upstream 时为 None。
    pub upstream: Option<String>,
    /// 本地领先 upstream 的提交数（可推送数）。
    pub ahead: usize,
    /// 本地落后 upstream 的提交数（可拉取数）。
    pub behind: usize,
}

/// `git status --porcelain=v1 -z` 的解析结果。
///
/// 路径按仓库根的相对路径存储（unix 分隔符），由调用方拼接工作目录转绝对路径。
#[derive(Debug, Default)]
pub struct GitStatus {
    pub statuses: Vec<(PathBuf, FileStatus)>,
    /// 分支头行（`-b` 输出）；首个头行优先，解析失败为 None（不阻断整体解析）。
    pub branch: Option<BranchStatus>,
}

/// 解析分支头行（去掉 `## ` 前缀后的内容），失败返回 None（保守跳过，不阻断 status 解析）。
fn parse_branch_header(header: &[u8]) -> Option<BranchStatus> {
    let text = std::str::from_utf8(header).ok()?;
    // 无 `...` → 无 upstream（`## main`、`## HEAD (no branch)`、`## No commits yet on main`）。
    let Some(upstream) = text.split_once("...").map(|(_, upstream)| upstream) else {
        return Some(BranchStatus {
            branch: parse_branch_name(text),
            ..Default::default()
        });
    };
    // 方括号段只在有 upstream 时出现：`origin/main [ahead 1, behind 2]` / `origin/main [gone]`。
    let (name, counts) = match upstream.find('[') {
        Some(index) => (&upstream[..index], Some(&upstream[index..])),
        None => (upstream, None),
    };
    let (ahead, behind) = match counts {
        Some(counts) if counts.starts_with("[gone]") => (0, 0),
        Some(counts) => parse_ahead_behind(counts)?,
        None => (0, 0),
    };
    Some(BranchStatus {
        branch: parse_branch_name(text),
        upstream: Some(name.trim().to_string()),
        ahead,
        behind,
    })
}

/// 提取分支名（`...` 前段；无 `...` 时取整段）。
///
/// detached HEAD（`HEAD (no branch)`）与空仓库（`No commits yet on <branch>`）无实际分支名，返回 None。
fn parse_branch_name(text: &str) -> Option<String> {
    let name = text
        .split_once("...")
        .map(|(name, _)| name)
        .unwrap_or(text)
        .trim();
    if name.is_empty() || name == "HEAD (no branch)" || name.starts_with("No commits yet on ") {
        return None;
    }
    Some(name.to_string())
}

/// 解析 `[ahead N, behind M]` 段：逐个找 `ahead `/`behind ` 前缀后的数字，缺的计 0。
fn parse_ahead_behind(counts: &str) -> Option<(usize, usize)> {
    let parse = |label: &str| {
        counts
            .find(label)
            .and_then(|index| {
                counts[index + label.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse::<usize>()
                    .ok()
            })
            .unwrap_or(0)
    };
    Some((parse("ahead "), parse("behind ")))
}

impl GitStatus {
    /// 解析 porcelain v1 -z 的原始输出。
    ///
    /// `-z` 模式下路径按原始字节输出（不转义），这里按 bytes 切分以兼容
    /// 非 UTF-8 路径；`--no-renames` 保证每项恰好两位状态码 + 空格 + 路径。
    pub fn from_bytes(output: &[u8]) -> Result<Self> {
        let mut statuses = Vec::new();
        let mut branch = None;
        for entry in output.split(|&byte| byte == b'\0') {
            if entry.is_empty() {
                continue;
            }
            // `-b` 分支头行：`## ` 会通过下方 `entry[2] == b' '` 守卫后按状态码解析而报错，
            // 必须在守卫前特判；首个头行优先（多仓库嵌套时外层先行）。
            if let Some(header) = entry.strip_prefix(b"## ") {
                branch = branch.or(parse_branch_header(header));
                continue;
            }
            anyhow::ensure!(
                entry.len() >= 3 && entry[2] == b' ',
                "无效的 git status 记录"
            );
            let mut path = &entry[3..];
            let is_dir = path.ends_with(b"/");
            // untracked 目录（`?? dir/`）跳过：目录汇总由消费方自行计算，
            // 且嵌套仓库的输出会干扰状态表；`--ignored=matching` 的忽略目录
            // （`!! dir/`）保留，路径去掉尾部 `/`（目录不展开的依据）。
            if is_dir && !entry.starts_with(b"!! ") {
                continue;
            }
            if is_dir {
                path = &path[..path.len() - 1];
            }
            let status = FileStatus::from_bytes([entry[0], entry[1]])?;
            statuses.push((crate::path_from_git_bytes(path), status));
        }
        statuses.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(Self { statuses, branch })
    }
}

/// 单文件的行数统计（`git diff --numstat`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub added: u64,
    pub deleted: u64,
}

/// 解析 `git diff --numstat -z` 输出，每项形如 `added\tdeleted\tpath\0`。
///
/// 二进制文件的行数计为 `-`，解析失败时跳过该行。
/// 路径按原始字节解析，兼容非 UTF-8。
pub(crate) fn parse_numstat(output: &[u8]) -> HashMap<PathBuf, DiffStat> {
    let mut entries = HashMap::new();
    for entry in output.split(|&byte| byte == b'\0') {
        if entry.is_empty() {
            continue;
        }
        let mut parts = entry.split(|&byte| byte == b'\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        let Ok(added) = parse_count(added) else {
            continue;
        };
        let Ok(deleted) = parse_count(deleted) else {
            continue;
        };
        entries.insert(
            crate::path_from_git_bytes(path),
            DiffStat { added, deleted },
        );
    }
    entries
}

/// numstat 的计数可能是 `-`（二进制文件），此时按 0 处理。
fn parse_count(bytes: &[u8]) -> Result<u64> {
    let text = std::str::from_utf8(bytes).context("numstat 计数非 UTF-8")?;
    if text == "-" {
        Ok(0)
    } else {
        text.parse::<u64>().context("numstat 计数非法")
    }
}

#[cfg(test)]
#[path = "test/status_tests.rs"]
mod tests;
