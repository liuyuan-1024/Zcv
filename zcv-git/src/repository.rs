//! git 命令行的同步封装：`.git` 目录解析与命令执行。
//!
//! 仓库发现（沿祖先查找 / 项目树内遍历）是项目管理层的决策，由 worktree 快照层负责（对齐 Zed：发现逻辑在 worktree crate，git crate 只做命令封装与输出解析）。
//! 所有方法同步阻塞执行，由调用方负责移入后台线程。

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::diff::DiffHunk;
use anyhow::{Context as _, Result, bail};

use crate::diff::parse_diff_hunks_per_path;
use crate::status::{DiffStat, GitStatus, parse_numstat};

/// 单个具体 git 进程的取消句柄。
///
/// 每次远程操作都持有全新句柄。进程编号与操作类型刻意分离，确保取消旧推送永远不会影响后续推送。
#[derive(Clone, Default)]
pub struct GitCancellation {
    inner: Arc<GitCancellationInner>,
}

#[derive(Default)]
struct GitCancellationInner {
    requested: AtomicBool,
    process_id: AtomicU32,
    progress: Mutex<Option<String>>,
}

impl GitCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.requested.load(Ordering::Acquire)
    }

    /// 中断整个 git 进程树；若未及时退出则强制终止。本方法不会阻塞界面线程。
    pub fn cancel(&self) {
        if self.inner.requested.swap(true, Ordering::AcqRel) {
            return;
        }
        let process_id = self.inner.process_id.load(Ordering::Acquire);
        if process_id == 0 {
            return;
        }
        self.begin_termination(process_id);
    }

    pub fn progress(&self) -> Option<String> {
        self.inner.progress.lock().ok()?.clone()
    }

    fn attach(&self, process_id: u32) {
        self.inner.process_id.store(process_id, Ordering::Release);
        if self.is_cancelled() {
            self.begin_termination(process_id);
        }
    }

    fn detach(&self, process_id: u32) {
        let _ = self.inner.process_id.compare_exchange(
            process_id,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn set_progress(&self, progress: String) {
        if let Ok(mut current) = self.inner.progress.lock() {
            *current = Some(progress);
        }
    }

    fn begin_termination(&self, process_id: u32) {
        interrupt_process_tree(process_id);
        let inner = Arc::clone(&self.inner);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            if inner.process_id.load(Ordering::Acquire) == process_id {
                kill_process_tree(process_id);
            }
        });
    }
}

fn read_stream<R: Read>(mut stream: R, progress: Option<GitCancellation>) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut pending_line = Vec::new();
    loop {
        let read = stream.read(&mut chunk).context("读取 git 输出失败")?;
        if read == 0 {
            break;
        }
        output.extend_from_slice(&chunk[..read]);
        if let Some(progress) = &progress {
            for byte in &chunk[..read] {
                if matches!(*byte, b'\r' | b'\n') {
                    publish_progress(progress, &pending_line);
                    pending_line.clear();
                } else {
                    pending_line.push(*byte);
                }
            }
        }
    }
    if let Some(progress) = &progress {
        publish_progress(progress, &pending_line);
    }
    Ok(output)
}

fn publish_progress(cancellation: &GitCancellation, line: &[u8]) {
    let line = String::from_utf8_lossy(line);
    let line = line.trim();
    if !line.is_empty() {
        cancellation.set_progress(line.to_string());
    }
}

#[cfg(unix)]
fn interrupt_process_tree(process_id: u32) {
    // 远程命令运行在独立进程组中，负进程号可以同时覆盖 git、ssh、凭据助手与钩子。
    unsafe {
        libc::kill(-(process_id as i32), libc::SIGINT);
    }
}

#[cfg(unix)]
fn kill_process_tree(process_id: u32) {
    unsafe {
        libc::kill(-(process_id as i32), libc::SIGKILL);
    }
}

#[cfg(windows)]
fn interrupt_process_tree(process_id: u32) {
    // Windows 后端接入作业对象前，以 taskkill /T 作为兼容方案；/F 仅留给强制终止阶段。
    let _ = Command::new("taskkill")
        .args(["/PID", &process_id.to_string(), "/T"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(windows)]
fn kill_process_tree(process_id: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &process_id.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(not(any(unix, windows)))]
fn interrupt_process_tree(_process_id: u32) {}

#[cfg(not(any(unix, windows)))]
fn kill_process_tree(_process_id: u32) {}

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

    /// 查询 diff 行数统计：`staged` 为 index↔HEAD（`--cached`），否则为 worktree↔index。
    fn diff_stat(&self, staged: bool, paths: &[PathBuf]) -> Result<HashMap<PathBuf, DiffStat>>;

    /// 批量查询多个路径相对 HEAD 的行级 diff hunks（单进程 `git diff --unified=0 HEAD -- <paths>`）。
    ///
    /// 未跟踪/干净/二进制文件不在结果中；空仓库（无 HEAD 提交）时返回空。
    /// base 取 HEAD（对齐 Zed 的 diff base：`HEAD:path` blob），staged + unstaged 合并显示。
    /// 单文件段解析失败仅跳过该路径，不中断整批。
    fn diff_hunks_for_paths(&self, paths: &[PathBuf]) -> Result<Vec<(PathBuf, Vec<DiffHunk>)>>;

    /// 批量读取 revision（如 `HEAD:path`、`:path`）的 blob 内容，缺失的 revision 为 `None`。
    fn load_revisions(&self, revs: &[&str]) -> Result<Vec<Option<Vec<u8>>>>;

    /// 拉取远程引用（`git fetch`，默认 remote/upstream）。
    fn fetch(&self) -> Result<()>;

    /// 可取消的远端同步。实现可覆盖该方法来终止底层进程树。
    fn fetch_cancellable(&self, cancellation: &GitCancellation) -> Result<()> {
        let _ = cancellation;
        self.fetch()
    }

    /// 拉取并合并当前分支（`git pull`，默认 upstream）。
    fn pull(&self) -> Result<()>;

    fn pull_cancellable(&self, cancellation: &GitCancellation) -> Result<()> {
        let _ = cancellation;
        self.pull()
    }

    /// 推送当前分支到上游（`git push`，默认 upstream）。
    fn push(&self) -> Result<()>;

    fn push_cancellable(&self, cancellation: &GitCancellation) -> Result<()> {
        let _ = cancellation;
        self.push()
    }

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

    /// HEAD 提交的 oid 与 subject（首行）一次查询（`git log -1 --pretty=format:%H%x00%s`）。
    ///
    /// 无提交（空仓库）时两项均为 `None`；oid 异常缺失时 subject 一并置 None（空仓库语义）。
    fn head_commit(&self) -> Result<(Option<String>, Option<String>)>;

    /// 撤销最近一次提交（先取完整消息，再 `git reset --soft HEAD^`，对齐 Zed uncommit）。
    ///
    /// 返回被撤销提交的完整消息（含 body，`%B`），供调用方填回提交信息编辑器；
    /// 无提交或撤销失败（如单提交仓库 `HEAD^` 不存在）时返回错误。
    fn uncommit(&self) -> Result<Option<String>>;

    /// 列出全部本地分支（`git for-each-ref --format=%(HEAD)%00%(refname:short) refs/heads`）。
    ///
    /// `%(HEAD)` 为 `*` 表示当前分支；空仓库返回空列表。
    fn branches(&self) -> Result<Vec<Branch>>;

    /// 切换到本地分支（`git checkout <name>`）。
    ///
    /// 先 `git show-ref --verify --quiet refs/heads/{name}` 验证：
    /// 防止列表生成到确认的窗口期分支被外部删除后，checkout 的 DWIM 意外创建远端跟踪分支（对齐 Zed change_branch）。
    fn checkout(&self, name: &str) -> Result<()>;

    /// 创建并切换到分支（`git switch -c <name> [base]`，对齐 Zed create_branch）。
    ///
    /// base 省略时从当前 HEAD 创建；空仓库（unborn HEAD）与 detached HEAD 均可用。
    fn create_branch(&self, name: &str, base: Option<&str>) -> Result<()>;
}

/// 单个本地分支（`git for-each-ref refs/heads` 的一行）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub is_head: bool,
}

pub struct RealGitRepository {
    working_directory: PathBuf,
}

impl RealGitRepository {
    /// 打开 `.git` 目录指向的仓库。
    ///
    /// `dot_git` 由调用方保证是目录（调用前已 `is_dir()` 检查）。
    /// v1 只支持普通仓库布局：worktree/子模块的 `.git` 文件指针、分仓库（separate git dir，commondir 文件）均不支持。
    pub fn open(dot_git: &Path) -> Result<Self> {
        let git_dir = dot_git
            .canonicalize()
            .with_context(|| format!("无法解析 .git 路径 {}", dot_git.display()))?;
        let working_directory = git_dir
            .parent()
            .context(".git 目录没有父目录")?
            .to_path_buf();
        Ok(Self { working_directory })
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

    /// 执行远程命令并保留进程身份，使界面线程发出的取消请求能够中断整个进程树。
    fn run_cancellable_command(
        &self,
        command: &mut Command,
        description: &str,
        cancellation: &GitCancellation,
    ) -> Result<Output> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }

        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("执行 git 命令失败")?;
        let process_id = child.id();
        cancellation.attach(process_id);

        let stdout = child.stdout.take().context("无法读取 git 标准输出")?;
        let stderr = child.stderr.take().context("无法读取 git 错误输出")?;
        let stdout_reader = std::thread::spawn(move || read_stream(stdout, None));
        let progress = cancellation.clone();
        let stderr_reader = std::thread::spawn(move || read_stream(stderr, Some(progress)));

        let status = child.wait().context("等待 git 命令结束失败")?;
        let stdout = stdout_reader
            .join()
            .map_err(|_| anyhow::anyhow!("读取 git 标准输出的线程异常退出"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| anyhow::anyhow!("读取 git 错误输出的线程异常退出"))??;
        // 读取线程结束意味着所有继承管道的子进程也已退出，此时才解除进程组绑定；
        // 否则 git 主进程先退出时会导致强制终止阶段漏掉仍存活的钩子子进程。
        cancellation.detach(process_id);
        let output = Output {
            status,
            stdout,
            stderr,
        };
        if !output.status.success() {
            if cancellation.is_cancelled() {
                bail!("{description} 已取消");
            }
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

    fn diff_hunks_for_paths(&self, paths: &[PathBuf]) -> Result<Vec<(PathBuf, Vec<DiffHunk>)>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        // quotepath=false：输出路径按原始字节而非 C 引用转义（控制字符路径除外，解析时跳过）。
        let mut command = self.build_command(&[
            "-c",
            "core.quotepath=false",
            "diff",
            "--unified=0",
            "HEAD",
            "--",
        ]);
        for path in paths {
            // 与单文件版本同模式：pathspec 原样传入，含空格/非 UTF-8 路径安全。
            command.arg(path);
        }
        let output = command
            .stdin(Stdio::null())
            .output()
            .context("执行 git diff --unified=0 失败")?;
        if !output.status.success() {
            // 空仓库（无 HEAD 提交）等"无结果"场景返回空，不报错。
            return Ok(Vec::new());
        }
        Ok(parse_diff_hunks_per_path(&output.stdout, paths))
    }

    fn fetch(&self) -> Result<()> {
        self.fetch_cancellable(&GitCancellation::new())
    }

    fn fetch_cancellable(&self, cancellation: &GitCancellation) -> Result<()> {
        self.run_cancellable_command(
            &mut self.build_command(&["fetch", "--progress"]),
            "git fetch",
            cancellation,
        )?;
        Ok(())
    }

    fn pull(&self) -> Result<()> {
        self.pull_cancellable(&GitCancellation::new())
    }

    fn pull_cancellable(&self, cancellation: &GitCancellation) -> Result<()> {
        self.run_cancellable_command(
            &mut self.build_command(&["pull", "--progress"]),
            "git pull",
            cancellation,
        )?;
        Ok(())
    }

    fn push(&self) -> Result<()> {
        self.push_cancellable(&GitCancellation::new())
    }

    fn push_cancellable(&self, cancellation: &GitCancellation) -> Result<()> {
        self.run_cancellable_command(
            &mut self.build_command(&["push", "--progress"]),
            "git push",
            cancellation,
        )?;
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

    fn head_commit(&self) -> Result<(Option<String>, Option<String>)> {
        // 空仓库（无提交）时 git log 非零退出，run_optional 置 None，不报错。
        let Some(output) = self.run_optional(&["log", "-1", "--pretty=format:%H%x00%s"])? else {
            return Ok((None, None));
        };
        let mut parts = output.stdout.splitn(2, |byte| *byte == 0);
        let oid = parts
            .next()
            .map(|oid| String::from_utf8_lossy(oid).trim().to_string())
            .filter(|oid| !oid.is_empty());
        let subject = parts
            .next()
            .map(|subject| String::from_utf8_lossy(subject).trim().to_string())
            .filter(|subject| !subject.is_empty());
        // oid 异常缺失时 subject 一并置 None，对齐 head() 的空仓库语义。
        Ok(if oid.is_some() {
            (oid, subject)
        } else {
            (None, None)
        })
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

    fn branches(&self) -> Result<Vec<Branch>> {
        // `refs/heads` 前缀匹配含嵌套分支（refs/heads/feature/x）；`refname:short` 直接给短名。
        let output = self.run_command(
            &mut self.build_command(&[
                "for-each-ref",
                "--format=%(HEAD)%00%(refname:short)",
                "refs/heads",
            ]),
            "git for-each-ref",
        )?;
        let mut branches = Vec::new();
        for line in output.stdout.split(|byte| *byte == b'\n') {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.splitn(2, |byte| *byte == 0);
            let is_head = parts.next().is_some_and(|head| head == b"*");
            if let Some(name) = parts.next() {
                let name = String::from_utf8_lossy(name);
                if !name.is_empty() {
                    branches.push(Branch {
                        name: name.into_owned(),
                        is_head,
                    });
                }
            }
        }
        Ok(branches)
    }

    fn checkout(&self, name: &str) -> Result<()> {
        let local_ref = format!("refs/heads/{name}");
        if self
            .run_optional(&["show-ref", "--verify", "--quiet", &local_ref])?
            .is_none()
        {
            bail!("本地分支 {name} 不存在");
        }
        self.run_command(&mut self.build_command(&["checkout", name]), "git checkout")?;
        Ok(())
    }

    fn create_branch(&self, name: &str, base: Option<&str>) -> Result<()> {
        let mut args = vec!["switch", "-c", name];
        if let Some(base) = base {
            args.push(base);
        }
        self.run_command(&mut self.build_command(&args), "git switch -c")?;
        Ok(())
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
#[path = "repository/test/repository_tests.rs"]
mod repository_tests;
