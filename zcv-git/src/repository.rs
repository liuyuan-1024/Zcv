//! git 命令行的同步封装：`.git` 目录解析与命令执行。
//!
//! 仓库发现（沿祖先查找 / 项目树内遍历）是项目管理层的决策，由 `zcv` 的 worktree 快照层负责（对齐 Zed：发现逻辑在 worktree crate，git crate 只做命令封装与输出解析）。
//! 所有方法同步阻塞执行，由调用方负责移入后台线程。

use crate::diff::parse_diff_hunks;
use crate::status::{DiffStat, GitStatus, parse_numstat};
use anyhow::{Context as _, Result, bail};
use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use zcv_buffer_diff::DiffHunk;

/// 对单个 git 仓库的命令行封装。
///
/// 全部方法同步阻塞，调用方需移到后台线程执行；失败统一返回带 stderr 的 anyhow 错误。
pub trait GitRepository: Send + Sync {
    /// 仓库工作目录（status 路径参数的基准）。
    fn working_directory(&self) -> &Path;

    /// 查询指定路径的 git 状态（`git status --porcelain=v1 -z`）。
    ///
    /// `paths` 为空时查询全仓库；路径相对仓库根（unix 分隔符），空字符串表示整棵工作树。
    fn status(&self, paths: &[PathBuf]) -> Result<GitStatus>;

    /// 是否配置了至少一个 remote（`git remote` 输出非空）。
    fn has_remote(&self) -> Result<bool>;

    /// 当前分支名与 HEAD 提交 id；空仓库或 detached HEAD 时对应项为 `None`。
    fn head(&self) -> Result<(Option<String>, Option<String>)>;

    /// 查询 diff 行数统计：`staged` 为 index↔HEAD（`--cached`），否则为 worktree↔index。
    fn diff_stat(&self, staged: bool, paths: &[PathBuf]) -> Result<HashMap<PathBuf, DiffStat>>;

    /// 查询单文件相对 HEAD 的行级 diff hunks（`git diff --unified=0 HEAD -- <path>`）。
    ///
    /// 未跟踪/干净/二进制文件输出为空；空仓库（无 HEAD 提交）时返回空。
    /// base 取 HEAD（对齐 Zed 的 diff base：`HEAD:path` blob），staged + unstaged 合并显示。
    fn diff_hunks(&self, path: &Path) -> Result<Vec<DiffHunk>>;

    /// 批量读取 revision（如 `HEAD:path`、`:path`）的 blob 内容，缺失的 revision 为 `None`。
    fn load_revisions(&self, revs: &[&str]) -> Result<Vec<Option<Vec<u8>>>>;

    /// 拉取远程引用（`git fetch`，默认 remote/upstream）。
    fn fetch(&self) -> Result<()>;

    /// 拉取并合并当前分支（`git pull`，默认 upstream）。
    fn pull(&self) -> Result<()>;

    /// 推送当前分支到上游（`git push`，默认 upstream）。
    fn push(&self) -> Result<()>;

    /// 暂存路径（`git update-index --add --remove -- <paths>`，对齐 Zed）。
    ///
    /// 路径相对仓库根；空列表为无操作。`--remove` 让已删除文件的删除进入 index。
    fn stage_paths(&self, paths: &[PathBuf]) -> Result<()>;

    /// 取消暂存路径（`git reset --quiet -- <paths>`，对齐 Zed）。
    ///
    /// 重置 index 到 HEAD；此前已暂存但 HEAD 中不存在的路径（新建后暂存）会移出 index。
    fn unstage_paths(&self, paths: &[PathBuf]) -> Result<()>;

    /// 提交暂存内容（`git commit --quiet -m <msg> --cleanup=strip`，对齐 Zed）。
    ///
    /// 消息经 `-m` 单参数原样传入（多行消息允许）；空消息 git 会报错，由调用方先校验。
    /// `--cleanup=strip` 丢弃消息注释行与行尾空白（对齐 Zed repository.rs:2532）。
    fn commit(&self, message: &str) -> Result<()>;

    /// 最近一次提交的 subject（首行，`git log -1 --pretty=format:%s`）。
    ///
    /// 无提交（空仓库）时为 `None`；对齐 Zed branch scan 的 `%(contents:subject)`。
    fn last_commit_message(&self) -> Result<Option<String>>;

    /// 撤销最近一次提交（先取完整消息，再 `git reset --soft HEAD^`，对齐 Zed uncommit）。
    ///
    /// 返回被撤销提交的完整消息（含 body，`%B`），供调用方填回提交信息编辑器；
    /// 无提交或撤销失败（如单提交仓库 `HEAD^` 不存在）时返回错误。
    fn uncommit(&self) -> Result<Option<String>>;
}

pub struct RealGitRepository {
    git_dir: PathBuf,
    working_directory: PathBuf,
}

impl RealGitRepository {
    /// 打开 `.git` 目录指向的仓库。
    ///
    /// `dot_git` 由调用方保证是目录（调用前已 `is_dir()` 检查）。
    /// v1 只支持
    /// 普通仓库布局：worktree/子模块的 `.git` 文件指针、分仓库（separate git dir，commondir 文件）均不支持。
    pub fn open(dot_git: &Path) -> Result<Self> {
        let git_dir = dot_git
            .canonicalize()
            .with_context(|| format!("无法解析 .git 路径 {}", dot_git.display()))?;
        let working_directory = git_dir
            .parent()
            .context(".git 目录没有父目录")?
            .to_path_buf();
        Ok(Self {
            git_dir,
            working_directory,
        })
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// 构造 git 命令。
    ///
    /// 固定参数对齐 Zed `build_command`（repository.rs:3695）：
    /// `--no-optional-locks` 防止 `git status` 回写 index（racy-git），避免"扫描 → fs 事件 → 再扫描"的自触发循环；`--no-pager` 防止交互式分页。
    fn build_command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .current_dir(&self.working_directory)
            .arg("-c")
            .arg("core.fsmonitor=false")
            .arg("-c")
            .arg("log.showSignature=false")
            .arg("--no-optional-locks")
            .arg("--no-pager")
            .args(args);
        command
    }

    /// 执行命令并断言成功；失败返回带 stderr 的错误。
    fn run_command(&self, command: &mut Command, description: &str) -> Result<Output> {
        let output = command
            .stdin(Stdio::null())
            .output()
            .context("执行 git 命令失败")?;
        if !output.status.success() {
            bail!(
                "{description} 失败：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output)
    }

    /// 执行命令，非零退出码视为"无结果"返回 `None`（不报错）。
    fn run_optional(&self, args: &[&str]) -> Result<Option<Output>> {
        let output = self
            .build_command(args)
            .stdin(Stdio::null())
            .output()
            .context("执行 git 命令失败")?;
        Ok(output.status.success().then_some(output))
    }
}

impl GitRepository for RealGitRepository {
    fn working_directory(&self) -> &Path {
        &self.working_directory
    }

    fn has_remote(&self) -> Result<bool> {
        // `git remote` 无 remote 时退出码仍为 0（输出为空），须按输出内容判定。
        Ok(self
            .run_optional(&["remote"])?
            .is_some_and(|output| output.stdout.iter().any(|byte| !byte.is_ascii_whitespace())))
    }

    fn status(&self, paths: &[PathBuf]) -> Result<GitStatus> {
        // 对齐 Zed `git_status_args`（repository.rs:3516），另加 `--ignored=matching`：
        // `--untracked-files=all` 让未跟踪文件逐条输出（非目录汇总）；
        // `--ignored=matching` 让被忽略的目录以 `!! dir/` 输出（不逐文件展开，避免 node_modules 这类目录撑爆输出），未被目录覆盖的忽略文件逐条输出；
        // `--no-renames` 保证每项恰两位状态码，`-z` 用 NUL 分隔原始字节路径。
        let mut command = self.build_command(&[
            "status",
            "--porcelain=v1",
            "-b",
            "--ignored=matching",
            "--untracked-files=all",
            "--no-renames",
            "-z",
            "--",
        ]);
        for path in paths {
            // 空路径表示整棵工作树，git 的约定是 `.`。
            let arg: &Path = if path.as_os_str().is_empty() {
                Path::new(".")
            } else {
                path
            };
            command.arg(arg);
        }
        let output = self.run_command(&mut command, "git status")?;
        GitStatus::from_bytes(&output.stdout)
    }

    fn head(&self) -> Result<(Option<String>, Option<String>)> {
        // symbolic-ref 在 detached HEAD 时非零退出；
        // rev-parse --verify 在空仓库时非零退出（裸 rev-parse HEAD 会输出字面量 "HEAD"）。
        let branch = self
            .run_optional(&["symbolic-ref", "--short", "-q", "HEAD"])?
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|name| !name.is_empty());
        let oid = self
            .run_optional(&["rev-parse", "--verify", "HEAD"])?
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|oid| !oid.is_empty());
        // 空仓库（无 HEAD 提交）时 symbolic-ref 仍输出分支名，但该分支尚不存在，没有实际意义，一并置 None。
        let branch = if oid.is_some() { branch } else { None };
        Ok((branch, oid))
    }

    fn diff_stat(&self, staged: bool, paths: &[PathBuf]) -> Result<HashMap<PathBuf, DiffStat>> {
        // 对齐 Zed diff_stat（repository.rs:2355）：
        // `--cached HEAD` 为 index↔HEAD，无额外参数为 worktree↔index；
        // `-z` 输出原始字节路径。
        let mut args = vec!["diff", "--numstat", "--no-renames", "-z"];
        if staged {
            args.push("--cached");
            args.push("HEAD");
        }
        let mut command = self.build_command(&args);
        if !paths.is_empty() {
            command.arg("--");
            for path in paths {
                command.arg(path);
            }
        }
        let output = self.run_command(&mut command, "git diff --numstat")?;
        Ok(parse_numstat(&output.stdout))
    }

    fn diff_hunks(&self, path: &Path) -> Result<Vec<DiffHunk>> {
        let mut command = self.build_command(&["diff", "--unified=0", "HEAD", "--"]);
        // 单参数传 Path（与 diff_stat 同模式），含空格/非 UTF-8 路径安全。
        command.arg(path);
        let output = command
            .stdin(Stdio::null())
            .output()
            .context("执行 git diff --unified=0 失败")?;
        if !output.status.success() {
            // 空仓库（无 HEAD 提交）等"无结果"场景返回空，不报错。
            return Ok(Vec::new());
        }
        Ok(parse_diff_hunks(&output.stdout))
    }

    fn fetch(&self) -> Result<()> {
        self.run_command(&mut self.build_command(&["fetch"]), "git fetch")?;
        Ok(())
    }

    fn pull(&self) -> Result<()> {
        self.run_command(&mut self.build_command(&["pull"]), "git pull")?;
        Ok(())
    }

    fn push(&self) -> Result<()> {
        self.run_command(&mut self.build_command(&["push"]), "git push")?;
        Ok(())
    }

    fn stage_paths(&self, paths: &[PathBuf]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut command = self.build_command(&["update-index", "--add", "--remove", "--"]);
        for path in paths {
            command.arg(path);
        }
        self.run_command(&mut command, "git update-index")?;
        Ok(())
    }

    fn unstage_paths(&self, paths: &[PathBuf]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut command = self.build_command(&["reset", "--quiet", "--"]);
        for path in paths {
            command.arg(path);
        }
        self.run_command(&mut command, "git reset")?;
        Ok(())
    }

    fn commit(&self, message: &str) -> Result<()> {
        // `-m` 单参数：多行消息整体作为一个参数传给 git。
        self.run_command(
            &mut self.build_command(&["commit", "--quiet", "-m", message, "--cleanup=strip"]),
            "git commit",
        )?;
        Ok(())
    }

    fn last_commit_message(&self) -> Result<Option<String>> {
        // 空仓库（无提交）时 git log 非零退出，run_optional 置 None，不报错。
        let subject = self
            .run_optional(&["log", "-1", "--pretty=format:%s"])?
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|subject| !subject.is_empty());
        Ok(subject)
    }

    fn uncommit(&self) -> Result<Option<String>> {
        // 先取完整消息再 reset：reset 后旧提交对象不再可达，消息须先落袋。
        let message = self
            .run_optional(&["log", "-1", "--pretty=format:%B"])?
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|message| !message.is_empty());
        // `--soft` 只回退 HEAD 指针，index 与工作树保留（对齐 Zed uncommit：`git reset HEAD^ --soft`）。
        self.run_command(
            &mut self.build_command(&["reset", "--soft", "HEAD^"]),
            "git reset --soft HEAD^",
        )?;
        Ok(message)
    }

    fn load_revisions(&self, revs: &[&str]) -> Result<Vec<Option<Vec<u8>>>> {
        // 单进程批量读取（对齐 Zed repository.rs:1820 的 cat-file --batch）：
        // stdin 逐行写 revision，按 header（`<oid> <type> <size>` 或 `<oid> missing`）读取对应大小的 blob。
        // 函数尾部显式 kill+wait，防止大对象读一半时进程残留阻塞管道。
        let mut child = self
            .build_command(&["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("启动 git cat-file --batch 失败")?;
        let mut stdin = child.stdin.take().context("无法获取 cat-file stdin")?;
        // ChildStdout 只实现 Read，read_until 需要 BufRead。
        let mut stdout = BufReader::new(child.stdout.take().context("无法获取 cat-file stdout")?);

        let mut result = Vec::with_capacity(revs.len());
        for rev in revs {
            stdin
                .write_all(rev.as_bytes())
                .context("写入 revision 失败")?;
            stdin.write_all(b"\n").context("写入 revision 失败")?;
            stdin.flush().context("刷新 stdin 失败")?;

            let mut header_line = Vec::new();
            stdout
                .read_until(b'\n', &mut header_line)
                .context("读取 cat-file header 失败")?;
            let header = String::from_utf8_lossy(&header_line);
            let header = header.trim();
            if header.is_empty() {
                bail!("cat-file 返回空 header");
            }
            if header.ends_with(" missing") {
                result.push(None);
                continue;
            }
            let mut parts = header.split_whitespace();
            parts.next();
            let object_type = parts.next().context("cat-file header 缺 object type")?;
            let size: usize = parts
                .next()
                .context("cat-file header 缺 object size")?
                .parse()
                .context("cat-file object size 非法")?;
            let mut content = vec![0u8; size];
            stdout
                .read_exact(&mut content)
                .context("读取 blob 内容失败")?;
            // 每个对象后跟一个换行。
            let mut newline = [0u8; 1];
            stdout
                .read_exact(&mut newline)
                .context("读取对象尾换行失败")?;
            // 只接受 blob（tree/commit 等没有可显示的文本内容）。
            result.push((object_type == "blob").then_some(content));
        }

        drop(stdin);
        // kill_on_drop 的同步等价：无论如何回收子进程。
        let _ = child.kill();
        let _ = child.wait();
        Ok(result)
    }
}

/// 在 `working_directory` 初始化 git 仓库（`git init -b <branch>`）。
///
/// 分支名先读 `git config --global init.defaultBranch`，未配置则用 `fallback_branch`（对齐 Zed fs.rs 的 git_init）。
/// `-b` 参数会覆盖 init.defaultBranch 的默认分支选择，因此必须显式传入。
/// init 前不存在仓库对象，故为自由函数而非 trait 方法；
/// 同步阻塞，由调用方负责移入后台线程。
pub fn init(working_directory: &Path, fallback_branch: &str) -> Result<()> {
    std::fs::create_dir_all(working_directory)?;
    let branch = resolve_branch(configured_default_branch()?.as_deref(), fallback_branch);
    let output = std::process::Command::new("git")
        .current_dir(working_directory)
        .args(["init", "-b", &branch])
        .stdin(Stdio::null())
        .output()
        .context("执行 git init 失败")?;
    if !output.status.success() {
        bail!(
            "git init 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// 读取全局 `init.defaultBranch`；未配置（非零退出）或输出空白视为无配置。
fn configured_default_branch() -> Result<Option<String>> {
    let output = std::process::Command::new("git")
        .args(["config", "--global", "--get", "init.defaultBranch"])
        .stdin(Stdio::null())
        .output()
        .context("读取 init.defaultBranch 失败")?;
    if !output.status.success() {
        return Ok(None);
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!branch.is_empty()).then_some(branch))
}

/// 纯函数：配置的分支名（空白视为无）→ 实际使用的分支名。
fn resolve_branch(configured: Option<&str>, fallback: &str) -> String {
    configured
        .filter(|branch| !branch.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::FileStatus;
    use crate::status::StatusCode;
    use tempfile::TempDir;

    /// 创建带一个初始提交的临时 git 仓库，返回 (仓库根, 目录句柄)。
    ///
    /// `-b master` 固定初始分支名，避免依赖本机 git 的 `init.defaultBranch` 配置。
    fn test_repo() -> (PathBuf, TempDir) {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let root = temp_dir.path().to_path_buf();
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        fs::write(root.join("tracked.txt"), "第一行\n第二行\n").expect("应写入初始文件");
        run_in(&root, &["git", "add", "tracked.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        (root, temp_dir)
    }

    fn run_in(dir: &Path, args: &[&str]) -> Output {
        let output = Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .output()
            .unwrap_or_else(|error| {
                panic!("命令 {:?} 在 {:?} 执行失败：{error}", args, dir);
            });
        assert!(
            output.status.success(),
            "命令 {:?} 失败：{}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn open_repo(root: &Path) -> RealGitRepository {
        RealGitRepository::open(&root.join(".git")).expect("open 应成功")
    }

    #[test]
    fn status_reports_all_states() {
        let (root, _temp) = test_repo();
        let repo = open_repo(&root);

        fs::write(root.join("tracked.txt"), "已修改\n").expect("应修改文件");
        fs::write(root.join("new.txt"), "新文件\n").expect("应新建文件");
        fs::remove_file(root.join("tracked.txt")).ok();
        fs::write(root.join("tracked.txt"), "已修改\n").expect("应重新写入");

        let status = repo.status(&[]).expect("status 应成功");
        let by_path: HashMap<_, _> = status
            .statuses
            .iter()
            .map(|(p, s)| (p.as_path(), s))
            .collect();

        assert!(
            by_path
                .get(Path::new("tracked.txt"))
                .expect("应有 tracked.txt")
                .is_modified()
        );
        assert!(
            by_path
                .get(Path::new("new.txt"))
                .expect("应有 new.txt")
                .is_untracked()
        );
    }

    #[test]
    fn status_accepts_path_prefixes() {
        let (root, _temp) = test_repo();
        let repo = open_repo(&root);
        fs::create_dir_all(root.join("src")).expect("应创建目录");
        fs::write(root.join("src/one.rs"), "1\n").expect("应创建文件");
        fs::write(root.join("two.rs"), "2\n").expect("应创建文件");

        let status = repo.status(&[PathBuf::from("src")]).expect("status 应成功");
        let paths: Vec<_> = status.statuses.iter().map(|(p, _)| p.clone()).collect();
        assert_eq!(paths, vec![PathBuf::from("src/one.rs")]);
    }

    #[test]
    fn head_returns_branch_and_oid() {
        let (root, _temp) = test_repo();
        let repo = open_repo(&root);

        let (branch, oid) = repo.head().expect("head 应成功");
        assert_eq!(branch.as_deref(), Some("master"));
        assert!(oid.is_some_and(|oid| !oid.is_empty()));
    }

    #[test]
    fn head_is_none_in_empty_repository() {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        run_in(temp_dir.path(), &["git", "init", "-q"]);
        let repo = open_repo(temp_dir.path());

        let (branch, oid) = repo.head().expect("head 应成功");
        assert!(branch.is_none());
        assert!(oid.is_none());
    }

    #[test]
    fn diff_stat_reports_staged_and_unstaged() {
        let (root, _temp) = test_repo();
        let repo = open_repo(&root);

        fs::write(root.join("tracked.txt"), "第一行\n第二行\n第三行\n").expect("应修改文件");
        run_in(&root, &["git", "add", "tracked.txt"]);
        fs::write(root.join("tracked.txt"), "第一行\n改动\n").expect("应再次修改");

        let staged = repo.diff_stat(true, &[]).expect("diff_stat 应成功");
        assert_eq!(
            staged.get(Path::new("tracked.txt")),
            Some(&DiffStat {
                added: 1,
                deleted: 0
            })
        );
        // 工作区相对 index："第二行"→"改动"（+1/-1）且"第三行"被删（-1）。
        let unstaged = repo.diff_stat(false, &[]).expect("diff_stat 应成功");
        assert_eq!(
            unstaged.get(Path::new("tracked.txt")),
            Some(&DiffStat {
                added: 1,
                deleted: 2
            })
        );
    }

    #[test]
    fn diff_stat_with_path_prefix() {
        let (root, _temp) = test_repo();
        let repo = open_repo(&root);
        // diff 只覆盖已跟踪文件，未跟踪文件先 add。
        fs::write(root.join("a.txt"), "x\n").expect("应创建文件");
        fs::write(root.join("b.txt"), "y\n").expect("应创建文件");
        run_in(&root, &["git", "add", "a.txt", "b.txt"]);
        fs::write(root.join("a.txt"), "x\nx\n").expect("应修改文件");

        let stats = repo
            .diff_stat(false, &[PathBuf::from("a.txt")])
            .expect("diff_stat 应成功");
        assert!(stats.contains_key(Path::new("a.txt")));
        assert!(!stats.contains_key(Path::new("b.txt")));
    }

    #[test]
    fn load_revisions_reads_head_and_index_contents() {
        let (root, _temp) = test_repo();
        let repo = open_repo(&root);

        let (_, head_oid) = repo.head().expect("head 应成功");
        let head_oid = head_oid.expect("应有 HEAD");

        let contents = repo
            .load_revisions(&[
                &format!("HEAD:tracked.txt"),
                &format!(":tracked.txt"),
                &format!("{head_oid}:tracked.txt"),
                "HEAD:missing.txt",
            ])
            .expect("load_revisions 应成功");

        let expected = "第一行\n第二行\n".as_bytes();
        assert_eq!(contents[0].as_deref(), Some(expected));
        assert_eq!(contents[1].as_deref(), Some(expected));
        assert_eq!(contents[2].as_deref(), Some(expected));
        assert_eq!(contents[3], None);
    }

    #[test]
    fn status_does_not_touch_index_mtime() {
        // --no-optional-locks 生效：扫描不应回写 index（racy-git 写回），
        // 否则会产生"扫描 → fs 事件 → 再扫描"的自触发循环。
        let (root, _temp) = test_repo();
        let repo = open_repo(&root);
        let index_path = root.join(".git/index");
        fs::write(root.join("tracked.txt"), "修改\n").expect("应修改文件");

        let before = fs::metadata(&index_path)
            .expect("index 应存在")
            .modified()
            .expect("mtime");
        repo.status(&[]).expect("status 应成功");
        let after = fs::metadata(&index_path)
            .expect("index 应存在")
            .modified()
            .expect("mtime");

        assert_eq!(before, after, "status 不应回写 index");
    }

    #[test]
    fn status_reports_ignored_entries_with_ignored_matching() {
        // --ignored=matching 语义：被忽略目录目录级输出（`!! dir/`），
        // 未被目录覆盖的忽略文件逐条输出（含 .git/info/exclude 命中），
        // 未跟踪文件仍逐条输出。
        let (root, _temp) = test_repo();
        let repo = open_repo(&root);

        fs::write(root.join(".gitignore"), "node_modules/\nignored.log\n")
            .expect("应写入 .gitignore");
        fs::create_dir_all(root.join("node_modules/pkg")).expect("应创建目录");
        fs::write(root.join("node_modules/pkg/index.js"), "x\n").expect("应创建文件");
        fs::write(root.join("ignored.log"), "y\n").expect("应创建文件");
        fs::write(root.join("new.txt"), "z\n").expect("应创建文件");
        fs::write(root.join("tracked.txt"), "修改\n").expect("应修改文件");

        let status = repo.status(&[]).expect("status 应成功");
        let by_path: HashMap<_, _> = status
            .statuses
            .iter()
            .map(|(p, s)| (p.as_path(), s))
            .collect();

        assert!(
            by_path
                .get(Path::new("node_modules"))
                .expect("忽略目录应有条目")
                .is_ignored()
        );
        assert!(
            by_path
                .get(Path::new("ignored.log"))
                .expect("忽略文件应有条目")
                .is_ignored()
        );
        assert!(
            by_path
                .get(Path::new("new.txt"))
                .expect("未跟踪文件应有条目")
                .is_untracked()
        );
        assert!(
            by_path
                .get(Path::new("tracked.txt"))
                .expect("已跟踪修改应有条目")
                .is_modified()
        );
        // 忽略目录内部不逐文件展开。
        assert!(!by_path.contains_key(Path::new("node_modules/pkg/index.js")));
    }

    #[test]
    fn status_reports_info_exclude_ignores() {
        // .git/info/exclude 是仓库级忽略，自解析 .gitignore 覆盖不到。
        let (root, _temp) = test_repo();
        let repo = open_repo(&root);
        fs::write(root.join(".git/info/exclude"), "secret.log\n").expect("应写入 exclude");
        fs::write(root.join("secret.log"), "x\n").expect("应创建文件");

        let status = repo.status(&[]).expect("status 应成功");
        let by_path: HashMap<_, _> = status
            .statuses
            .iter()
            .map(|(p, s)| (p.as_path(), s))
            .collect();
        assert!(
            by_path
                .get(Path::new("secret.log"))
                .expect("info/exclude 命中应有条目")
                .is_ignored()
        );
    }

    #[test]
    fn status_with_path_prefix_reports_file_level_ignored() {
        // 增量刷新传具体文件路径时，ignored 按文件级输出。
        let (root, _temp) = test_repo();
        let repo = open_repo(&root);
        fs::write(root.join(".gitignore"), "ignored.log\n").expect("应写入 .gitignore");
        fs::write(root.join("ignored.log"), "y\n").expect("应创建文件");

        let status = repo
            .status(&[PathBuf::from("ignored.log")])
            .expect("status 应成功");
        let by_path: HashMap<_, _> = status
            .statuses
            .iter()
            .map(|(p, s)| (p.as_path(), s))
            .collect();
        assert!(
            by_path
                .get(Path::new("ignored.log"))
                .expect("路径参数下忽略文件应有条目")
                .is_ignored()
        );
    }

    /// 创建「本地裸远程 + 已推送初始提交的工作仓库」对，返回 (工作仓库根, 裸远程根, 临时目录句柄)。
    ///
    /// 工作仓库与裸远程共用同一个 temp_dir，保证返回后目录仍存活。
    fn test_repo_with_remote() -> (PathBuf, PathBuf, TempDir) {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let remote = temp_dir.path().join("remote.git");
        run_in(
            temp_dir.path(),
            &["git", "init", "-q", "--bare", remote.to_str().unwrap()],
        );
        let root = temp_dir.path().join("work");
        std::fs::create_dir(&root).expect("应创建工作仓库目录");
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        std::fs::write(root.join("tracked.txt"), "第一行\n第二行\n").expect("应写入初始文件");
        run_in(&root, &["git", "add", "tracked.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        run_in(
            &root,
            &["git", "remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&root, &["git", "push", "-q", "-u", "origin", "master"]);
        (root, remote, temp_dir)
    }

    #[test]
    fn fetch_pull_push_work_against_remote() {
        let (root, remote, _temp) = test_repo_with_remote();
        let repo = open_repo(&root);

        // 制造"本地领先远程一个提交"：提交后 push，再回退本地 HEAD。
        std::fs::write(root.join("pushed.txt"), "推送内容\n").expect("应写入文件");
        run_in(&root, &["git", "add", "pushed.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "待推送提交"]);
        run_in(&root, &["git", "push", "-q", "origin", "master"]);
        run_in(&root, &["git", "reset", "-q", "--hard", "HEAD~1"]);

        // fetch：本地引用更新为远程状态，工作树不动。
        repo.fetch().expect("fetch 应成功");
        let ahead = run_in(
            &root,
            &["git", "rev-list", "--count", "HEAD..origin/master"],
        );
        assert_eq!(
            String::from_utf8_lossy(&ahead.stdout).trim(),
            "1",
            "fetch 后本地应落后远程一个提交"
        );

        // pull：合并远程提交，本地追上远程。
        repo.pull().expect("pull 应成功");
        let behind = run_in(
            &root,
            &["git", "rev-list", "--count", "HEAD..origin/master"],
        );
        assert_eq!(
            String::from_utf8_lossy(&behind.stdout).trim(),
            "0",
            "pull 后本地应追上远程"
        );
        assert!(
            root.join("pushed.txt").exists(),
            "pull 应带下远程提交的文件"
        );

        // push：把新提交推送到远程。
        std::fs::write(root.join("again.txt"), "再推一次\n").expect("应写入文件");
        run_in(&root, &["git", "add", "again.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "再推一次"]);
        repo.push().expect("push 应成功");
        // 从远程裸仓库验证提交已到达（bare 仓库 HEAD 指向 master）。
        let remote_head = run_in(&remote, &["git", "rev-parse", "master"]);
        let local_head = run_in(&root, &["git", "rev-parse", "HEAD"]);
        assert_eq!(
            String::from_utf8_lossy(&remote_head.stdout).trim(),
            String::from_utf8_lossy(&local_head.stdout).trim(),
            "push 后远程应指向本地 HEAD"
        );
    }

    #[test]
    fn has_remote_reports_true_and_false() {
        let (root, _temp) = test_repo();
        assert!(!open_repo(&root).has_remote().expect("has_remote 应成功"));

        let (root, _remote, _temp) = test_repo_with_remote();
        assert!(open_repo(&root).has_remote().expect("has_remote 应成功"));
    }

    #[test]
    fn status_reports_branch_tracking() {
        let (root, _remote, _temp) = test_repo_with_remote();
        let repo = open_repo(&root);

        // 与远程同步：无 ahead/behind。
        let branch = repo
            .status(&[])
            .expect("status 应成功")
            .branch
            .expect("应有头行");
        assert_eq!(branch.upstream.as_deref(), Some("origin/master"));
        assert_eq!((branch.ahead, branch.behind), (0, 0));

        // 本地新提交 → ahead 1（可推送）。
        std::fs::write(root.join("new.txt"), "新提交\n").expect("应写入文件");
        run_in(&root, &["git", "add", "new.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "本地提交"]);
        let branch = repo
            .status(&[])
            .expect("status 应成功")
            .branch
            .expect("应有头行");
        assert_eq!((branch.ahead, branch.behind), (1, 0));

        // push 后回到同步（徽标消失的依据）。
        repo.push().expect("push 应成功");
        let branch = repo
            .status(&[])
            .expect("status 应成功")
            .branch
            .expect("应有头行");
        assert_eq!((branch.ahead, branch.behind), (0, 0));
    }

    #[test]
    fn diff_hunks_reports_worktree_changes() {
        use zcv_buffer_diff::DiffHunkKind::*;

        let (root, _temp) = test_repo();
        let repository = open_repo(&root);
        let tracked = Path::new("tracked.txt");

        // 干净文件：无 hunks。
        assert_eq!(repository.diff_hunks(tracked).unwrap(), vec![]);

        // 修改第 2 行 → Modified（range 1..2）。
        fs::write(root.join("tracked.txt"), "第一行\n改了第二行\n").expect("应写入文件");
        assert_eq!(
            repository.diff_hunks(tracked).unwrap(),
            vec![DiffHunk {
                range: 1..2,
                old_range: 1..2,
                kind: Modified,
            }]
        );

        // 末尾追加 → Added。
        fs::write(root.join("tracked.txt"), "第一行\n第二行\n新增行\n").expect("应写入文件");
        assert_eq!(
            repository.diff_hunks(tracked).unwrap(),
            vec![DiffHunk {
                range: 2..3,
                old_range: 1..1,
                kind: Added,
            }]
        );

        // 删除第一行 → Deleted（锚定删除点行 0）。
        fs::write(root.join("tracked.txt"), "第二行\n").expect("应写入文件");
        assert_eq!(
            repository.diff_hunks(tracked).unwrap(),
            vec![DiffHunk {
                range: 0..0,
                old_range: 0..1,
                kind: Deleted,
            }]
        );
    }

    #[test]
    fn diff_hunks_empty_for_untracked_clean_and_binary() {
        let (root, _temp) = test_repo();
        let repository = open_repo(&root);

        // 未跟踪文件：git diff HEAD 无输出。
        fs::write(root.join("untracked.txt"), "新的\n").expect("应写入文件");
        assert_eq!(
            repository.diff_hunks(Path::new("untracked.txt")).unwrap(),
            vec![]
        );

        // 干净文件：无 hunks。
        assert_eq!(
            repository.diff_hunks(Path::new("tracked.txt")).unwrap(),
            vec![]
        );

        // 已跟踪二进制文件：Binary files differ → 空。
        fs::write(root.join("img.png"), [0x89u8, 0x50, 0x4e, 0x47]).expect("应写入文件");
        run_in(&root, &["git", "add", "img.png"]);
        run_in(&root, &["git", "commit", "-q", "-m", "add png"]);
        fs::write(root.join("img.png"), [0x89u8, 0x50, 0x4e, 0x47, 0x00]).expect("应写入文件");
        assert_eq!(repository.diff_hunks(Path::new("img.png")).unwrap(), vec![]);
    }

    #[test]
    fn diff_hunks_empty_without_head() {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let root = temp_dir.path().to_path_buf();
        run_in(&root, &["git", "init", "-q", "-b", "master"]);

        let repository = open_repo(&root);
        assert_eq!(
            repository.diff_hunks(Path::new("tracked.txt")).unwrap(),
            vec![]
        );
    }

    /// 提取 FileStatus 的 index 状态（非 Tracked 视为 Unmodified）。
    fn index_status(status: &FileStatus) -> StatusCode {
        match status {
            FileStatus::Tracked { index_status, .. } => *index_status,
            _ => StatusCode::Unmodified,
        }
    }

    #[test]
    fn stage_and_unstage_paths_move_index_state() {
        let (root, _temp) = test_repo();
        let repository = open_repo(&root);
        let tracked = Path::new("tracked.txt");
        let new_file = Path::new("new.txt");

        // 修改已跟踪文件 + 新建文件。
        fs::write(root.join("tracked.txt"), "修改后的内容\n").expect("应修改文件");
        fs::write(root.join("new.txt"), "新文件\n").expect("应新建文件");

        // 初始：均已未暂存（index 未动）。
        let status = repository.status(&[]).expect("status 应成功");
        let by_path: HashMap<_, _> = status
            .statuses
            .iter()
            .map(|(path, status)| (path.as_path(), status))
            .collect();
        assert_eq!(index_status(by_path[tracked]), StatusCode::Unmodified);
        assert!(by_path[tracked].is_modified());
        assert!(by_path[new_file].is_untracked());

        // 暂存两个路径 → index 出现对应状态（修改 + 新增）。
        repository
            .stage_paths(&[tracked.to_path_buf(), new_file.to_path_buf()])
            .expect("stage 应成功");
        let status = repository.status(&[]).expect("status 应成功");
        let by_path: HashMap<_, _> = status
            .statuses
            .iter()
            .map(|(path, status)| (path.as_path(), status))
            .collect();
        assert_eq!(index_status(by_path[tracked]), StatusCode::Modified);
        assert_eq!(index_status(by_path[new_file]), StatusCode::Added);

        // 取消暂存 → 回到未暂存/未跟踪（新建文件移出 index）。
        repository
            .unstage_paths(&[tracked.to_path_buf(), new_file.to_path_buf()])
            .expect("unstage 应成功");
        let status = repository.status(&[]).expect("status 应成功");
        let by_path: HashMap<_, _> = status
            .statuses
            .iter()
            .map(|(path, status)| (path.as_path(), status))
            .collect();
        assert_eq!(index_status(by_path[tracked]), StatusCode::Unmodified);
        assert!(by_path[new_file].is_untracked());
    }

    #[test]
    fn init_creates_repository_with_configured_branch() {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let root = temp_dir.path().to_path_buf();

        init(&root, "main").expect("init 应成功");
        assert!(root.join(".git").is_dir(), "应创建 .git 目录");

        // 期望分支由本机全局 init.defaultBranch 决定（未配置则用 fallback），
        // 与实现同路径计算，避免测试依赖具体机器配置。
        let expected = resolve_branch(
            configured_default_branch()
                .expect("读取配置应成功")
                .as_deref(),
            "main",
        );
        let head = run_in(&root, &["git", "symbolic-ref", "--short", "HEAD"]);
        assert_eq!(
            String::from_utf8_lossy(&head.stdout).trim(),
            expected,
            "初始分支应使用 init.defaultBranch 或 fallback"
        );
    }

    #[test]
    fn resolve_branch_uses_configured_or_fallback() {
        assert_eq!(resolve_branch(None, "main"), "main");
        assert_eq!(resolve_branch(Some("dev"), "main"), "dev");
        assert_eq!(resolve_branch(Some("  "), "main"), "main");
    }

    #[test]
    fn commit_creates_commit_and_reports_subject() {
        let (root, _temp) = test_repo();
        let repository = open_repo(&root);

        fs::write(root.join("tracked.txt"), "修改后的内容\n").expect("应修改文件");
        repository
            .stage_paths(&[PathBuf::from("tracked.txt")])
            .expect("stage 应成功");
        // 标准提交消息：subject + 空行 + body；%s 只取首行。
        repository
            .commit("首行提交\n\n第二行详细说明")
            .expect("commit 应成功");

        // subject 只取首行（对齐 Zed 的 %(contents:subject)）。
        assert_eq!(
            repository
                .last_commit_message()
                .expect("查询应成功")
                .as_deref(),
            Some("首行提交")
        );
        // 与直接执行 git 对照。
        let log = run_in(&root, &["git", "log", "-1", "--pretty=format:%s"]);
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "首行提交");
    }

    #[test]
    fn last_commit_message_none_without_head() {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        run_in(temp_dir.path(), &["git", "init", "-q", "-b", "master"]);
        let repository = open_repo(temp_dir.path());

        assert_eq!(repository.last_commit_message().expect("查询应成功"), None);
    }

    #[test]
    fn commit_fails_without_changes() {
        let (root, _temp) = test_repo();
        let repository = open_repo(&root);

        // 干净工作树：git 报 nothing to commit，run_command 带 stderr 返回错误。
        assert!(repository.commit("无改动提交").is_err());
    }

    #[test]
    fn uncommit_returns_full_message_and_rewinds_head() {
        let (root, _temp) = test_repo();
        let repository = open_repo(&root);

        fs::write(root.join("tracked.txt"), "第二次修改\n").expect("应修改文件");
        repository
            .stage_paths(&[PathBuf::from("tracked.txt")])
            .expect("stage 应成功");
        repository
            .commit("第二次提交\n\n详细说明")
            .expect("commit 应成功");

        // uncommit 返回被撤销提交的完整消息（含 body，%B 保留空行），HEAD 回退到 initial。
        let message = repository
            .uncommit()
            .expect("uncommit 应成功")
            .expect("应返回被撤销提交的消息");
        assert_eq!(message, "第二次提交\n\n详细说明");
        let log = run_in(&root, &["git", "log", "-1", "--pretty=format:%s"]);
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "initial");

        // --soft：改动保留在 index（staged）。
        let status = repository.status(&[]).expect("status 应成功");
        let by_path: HashMap<_, _> = status
            .statuses
            .iter()
            .map(|(path, status)| (path.as_path(), status))
            .collect();
        assert_eq!(
            index_status(by_path[Path::new("tracked.txt")]),
            StatusCode::Modified
        );
    }
}
