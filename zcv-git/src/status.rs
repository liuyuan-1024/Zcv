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
    /// 对齐 Zed `entry_git_aware_label_color` 的判定顺序（editor/items.rs:2200）；
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
            statuses.push((path_from_bytes(path), status));
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
/// 二进制文件的行数计为 `-`，解析失败时跳过该行（与 Zed 行为一致）。
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
        entries.insert(path_from_bytes(path), DiffStat { added, deleted });
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

/// 由原始字节构造路径（git 输出为 unix 风格相对路径）。
#[cfg(unix)]
pub(crate) fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
pub(crate) fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
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
            let status =
                GitStatus::from_bytes(format!("{header}\0").as_bytes()).expect("应解析成功");
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
}
