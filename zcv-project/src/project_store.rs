//! 项目级状态与服务协调。
//!
//! `Project` 管理项目根、目录快照（Worktree）、文件 Buffer 生命周期和文件系统监听。
//! 窗口布局、Pane、Dock 与其他界面状态仍由 `Workspace` 管理。

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use gpui::{App, AppContext, AsyncApp, Context, Entity, EventEmitter, Task, WeakEntity};
use zcv_fs_watch::{FsWatcher, PathEvent, PathEventKind, Watcher};
use zcv_git::FileStatus;
use zcv_multi_buffer::MultiBuffer;
use zcv_text::{Buffer, BufferLoadError, BufferSaveError, SearchQuery};

use super::buffer_store::BufferStore;
use super::git_store::{GitStore, StatusEntry};
use super::search::{self, SearchResults};
use super::worktree::{Worktree, WorktreeEntry, collect_visible_entries};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectEvent {
    RootChanged(PathBuf),
    EntriesChanged,
}

/// 当前活动项目根（装配层注册：打开项目时设置、RootChanged 时更新）。
///
/// 供 breadcrumbs 等显示层做根相对化查询，避免把项目根概念渗入可嵌入组件状态。
#[derive(Clone, Debug)]
pub struct ActiveProjectRoot(pub Option<PathBuf>);

impl gpui::Global for ActiveProjectRoot {}

pub struct Project {
    /// 与 Zed 的空 WorktreeStore 语义一致：Project 始终存在，worktree 可以为空。
    worktree: Option<ProjectWorktree>,
    /// 与 Zed 一致：git store 属于 Project 而非 worktree，无 worktree 时以无根状态存在（仓库查询与 git job 为空操作）。
    git_store: Entity<GitStore>,
    buffer_store: BufferStore,
}

struct ProjectWorktree {
    root: PathBuf,
    snapshot: Worktree,
    fs_watcher: Arc<dyn Watcher>,
    _fs_task: Task<()>,
}

impl Project {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let fs_watcher = Arc::new(FsWatcher::new());
        let fs_events = fs_watcher.events();
        let fs_watcher: Arc<dyn Watcher> = fs_watcher;

        if let Err(error) = fs_watcher.add(&root) {
            eprintln!("无法监听项目目录 {:?}：{error}", root);
        }

        let fs_task = cx.spawn(|project: WeakEntity<Project>, asynccx: &mut AsyncApp| {
            let mut cx = asynccx.clone();
            async move {
                while let Some(events) = fs_events.next_batch().await {
                    let _ = project.update(&mut cx, |project, cx| {
                        project.process_fs_events(events, cx);
                    });
                }
            }
        });

        let git_store = cx.new(|cx| GitStore::new(Some(root.clone()), cx));
        git_store.update(cx, |store, cx| store.schedule_scan(cx));

        Self {
            worktree: Some(ProjectWorktree {
                root: root.clone(),
                snapshot: Worktree::new(root),
                fs_watcher,
                _fs_task: fs_task,
            }),
            git_store,
            buffer_store: BufferStore::new(),
        }
    }

    /// 创建没有 worktree 的本地项目，供空工作区使用。
    pub fn empty(cx: &mut Context<Self>) -> Self {
        let git_store = cx.new(|cx| GitStore::new(None, cx));
        Self {
            worktree: None,
            git_store,
            buffer_store: BufferStore::new(),
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.worktree
            .as_ref()
            .map(|worktree| worktree.root.as_path())
    }

    pub fn has_worktree(&self) -> bool {
        self.worktree.is_some()
    }

    /// 更新项目树的扫描排除规则（设置变化时由项目树调用）。
    pub fn set_exclusions(&mut self, exclusions: &[String]) {
        if let Some(worktree) = &mut self.worktree {
            worktree.snapshot.set_exclusions(exclusions);
        }
    }

    /// 后台收集当前展开状态下的可见行（不含 git 状态），返回后台任务。
    ///
    /// 遍历在后台线程执行（排序与排除规则与 `Worktree::children` 一致）；
    /// 完成后由调用方在 UI 线程用 `git_statuses_for_rows` 批量回填 git 状态。
    pub fn collect_visible_rows(
        &self,
        expanded: HashSet<PathBuf>,
        cx: &App,
    ) -> Task<Vec<WorktreeEntry>> {
        let Some(worktree) = &self.worktree else {
            // 无 worktree 的空态：直接返回空结果。
            return cx
                .background_executor()
                .spawn(async { Vec::<WorktreeEntry>::new() });
        };
        let root = worktree.root.clone();
        let filter = worktree.snapshot.filter();
        cx.background_executor()
            .spawn(async move { collect_visible_entries(&root, &expanded, &filter) })
    }

    /// 批量查询可见行的 git 状态（git 事件驱动，不重扫目录）。
    ///
    /// `rows` 为 (路径, 是否目录) 对：目录行取聚合状态，文件行取精确状态。
    pub fn git_statuses_for_rows(
        &self,
        rows: &[(PathBuf, bool)],
        cx: &App,
    ) -> HashMap<PathBuf, FileStatus> {
        rows.iter()
            .filter_map(|(path, is_dir)| {
                let status = if *is_dir {
                    self.git_status_for_directory(path, cx)
                } else {
                    self.git_status_for_path(path, cx).map(|entry| entry.status)
                };
                status.map(|status| (path.clone(), status))
            })
            .collect()
    }

    pub fn git_store(&self) -> Entity<GitStore> {
        self.git_store.clone()
    }

    /// 仅在有 worktree 时返回 git store（无 worktree 的项目不做 git 操作）。
    pub fn try_git_store(&self) -> Option<Entity<GitStore>> {
        self.has_worktree().then_some(self.git_store.clone())
    }

    pub fn open_buffer(
        &mut self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Result<Entity<MultiBuffer>, BufferLoadError> {
        self.buffer_store.open_buffer(path, cx)
    }

    /// 在后台逐文件扫描 worktree 并预加载命中文件，
    /// 结果经流式通道产出，由 UI 线程按批装配进 MultiBuffer。
    pub fn search(&mut self, query: SearchQuery, cx: &mut Context<Self>) -> SearchResults {
        let Some(worktree) = &self.worktree else {
            return SearchResults::empty();
        };
        let plan = worktree.snapshot.search_plan();
        let opened_snapshots = self.buffer_store.opened_snapshots(cx);
        let (tx, rx) = async_channel::bounded(8);
        let task = cx.background_executor().spawn(async move {
            let _ = search::search_worktree(plan, opened_snapshots, query, tx).await;
        });
        SearchResults { task, rx }
    }

    /// 注册搜索任务在后台加载完成的 Buffer，与已打开文档共享同一缓存。
    pub fn register_loaded_buffer(
        &mut self,
        path: PathBuf,
        buffer: Buffer,
        cx: &mut Context<Self>,
    ) -> Result<Entity<MultiBuffer>, BufferLoadError> {
        self.buffer_store.register_loaded_buffer(path, buffer, cx)
    }

    pub fn save_buffer(
        &mut self,
        multi_buffer: &Entity<MultiBuffer>,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Result<(), BufferSaveError> {
        let buffer = multi_buffer
            .read(cx)
            .as_singleton(cx)
            .expect("当前 Project 只保存 singleton MultiBuffer");
        self.save_file_buffers(vec![(buffer, path.to_path_buf())], cx)
    }

    /// 保存 MultiBuffer 引用的真实源文件 Buffer；
    /// 组合投影不会参与落盘。
    pub fn save_file_buffers(
        &mut self,
        buffers: Vec<(Entity<Buffer>, PathBuf)>,
        cx: &mut Context<Self>,
    ) -> Result<(), BufferSaveError> {
        let mut saved_paths = Vec::with_capacity(buffers.len());
        for (buffer, path) in buffers {
            buffer.update(cx, |buffer, cx| {
                write_buffer_to_path(buffer, &path)?;
                cx.notify();
                Ok::<_, BufferSaveError>(())
            })?;
            saved_paths.push(path);
        }
        // 保存成功后立即刷新 git 状态（快路径，不等 fs 事件；
        // fs 事件晚到会被 job 去重吸收）。
        if !saved_paths.is_empty() {
            self.git_store.update(cx, |store, cx| {
                store.refresh_statuses_for_paths(&saved_paths, cx);
            });
        }
        Ok(())
    }

    /// 在同一父目录内重命名文件或目录，并迁移项目持有的路径状态。
    pub fn rename_path(
        &mut self,
        from: &Path,
        to: &Path,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(from != to, "新旧路径不能相同");
        anyhow::ensure!(from.parent() == to.parent(), "重命名不能移动条目");
        let worktree = self
            .worktree
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("当前项目没有 worktree"))?;
        anyhow::ensure!(
            from == worktree.root || from.starts_with(&worktree.root),
            "条目不在当前项目中"
        );
        let indexed_from = from.canonicalize()?;
        if to.exists() {
            anyhow::ensure!(
                to.canonicalize()? == indexed_from,
                "目标已存在：{}",
                to.display()
            );
        }
        let indexed_to = indexed_from
            .parent()
            .and_then(|parent| to.file_name().map(|name| parent.join(name)))
            .ok_or_else(|| anyhow::anyhow!("无法确定重命名目标路径"))?;
        std::fs::rename(from, to)?;
        self.buffer_store.rename_path(&indexed_from, &indexed_to);

        if from == worktree.root {
            if let Err(error) = worktree.fs_watcher.add(to) {
                eprintln!("无法监听重命名后的项目目录 {:?}：{error}", to);
            }
            if let Err(error) = worktree.fs_watcher.remove(from) {
                eprintln!("无法停止监听旧项目目录 {:?}：{error}", from);
            }
            worktree.root = to.to_path_buf();
            worktree.snapshot.set_root(to.to_path_buf());
            cx.emit(ProjectEvent::RootChanged(to.to_path_buf()));
        } else {
            cx.emit(ProjectEvent::EntriesChanged);
        }
        Ok(())
    }

    /// 在项目内新建一个空文件或目录，并补齐缺失的父目录。
    pub fn create_path(
        &mut self,
        path: &Path,
        is_dir: bool,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let root = self
            .root()
            .ok_or_else(|| anyhow::anyhow!("当前项目没有 worktree"))?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| anyhow::anyhow!("条目不在当前项目中"))?;
        anyhow::ensure!(
            relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
            "条目路径不安全：{}",
            path.display()
        );
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("条目没有父目录"))?;
        anyhow::ensure!(!path.exists(), "目标已存在：{}", path.display());

        std::fs::create_dir_all(parent)?;
        if is_dir {
            std::fs::create_dir(path)?;
        } else {
            OpenOptions::new().write(true).create_new(true).open(path)?;
        }
        cx.emit(ProjectEvent::EntriesChanged);
        Ok(())
    }

    /// 将文件或目录移到系统废纸篓（可恢复），并清掉项目持有的路径状态。
    pub fn trash_path(&mut self, path: &Path, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let root = self
            .root()
            .ok_or_else(|| anyhow::anyhow!("当前项目没有 worktree"))?;
        anyhow::ensure!(path != root, "不能删除项目根目录");
        anyhow::ensure!(path.starts_with(root), "条目不在当前项目中");
        trash::delete(path)?;
        self.buffer_store.remove_path(path);
        cx.emit(ProjectEvent::EntriesChanged);
        Ok(())
    }

    /// 在项目内移动文件或目录到新位置（可跨目录），并迁移项目持有的路径状态。
    ///
    /// 与 `rename_path` 的区别：不要求同父目录；`overwrite` 为真时允许替换已存在的目标。
    pub fn move_path(
        &mut self,
        from: &Path,
        to: &Path,
        overwrite: bool,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(from != to, "新旧路径不能相同");
        let root = self
            .root()
            .ok_or_else(|| anyhow::anyhow!("当前项目没有 worktree"))?
            .to_path_buf();
        anyhow::ensure!(from != root, "不能移动项目根目录");
        anyhow::ensure!(from.starts_with(&root), "条目不在当前项目中");
        anyhow::ensure!(to.starts_with(&root), "目标不在当前项目中");
        anyhow::ensure!(!to.starts_with(from), "不能把条目移动到自身内部");
        // 对称守卫：目标是源的祖先目录时，覆盖路径的「先删目标」会把源一起递归删掉。
        anyhow::ensure!(
            !from.starts_with(to),
            "不能把条目移动到自身的祖先目录：{}",
            to.display()
        );
        if to.exists() {
            anyhow::ensure!(overwrite, "目标已存在：{}", to.display());
        }
        let indexed_from = from.canonicalize()?;
        let parent = to
            .parent()
            .ok_or_else(|| anyhow::anyhow!("无法确定移动目标路径"))?;
        let name = to
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("无法确定移动目标路径"))?;
        // 目标父目录必须已存在：目录移动场景下由调用方保证，缺失时在 canonicalize 处报错。
        let indexed_to = parent.canonicalize()?.join(name);
        // 优先直接 rename：同文件系统上 POSIX rename 原子替换文件/空目录目标，没有「先删后写」的危险中间态；
        // 仅当失败且目标是非空目录（rename 无法原地替换的唯一情形）才退化为「删目标再 rename」，最终失败经 Result 向上传播。
        if let Err(error) = std::fs::rename(from, to) {
            let is_nonempty_dir = to.is_dir()
                && std::fs::read_dir(to).is_ok_and(|mut entries| entries.next().is_some());
            if !(overwrite && is_nonempty_dir) {
                return Err(error)
                    .with_context(|| format!("移动失败：{} → {}", from.display(), to.display()));
            }
            remove_entry(to)?;
            std::fs::rename(from, to)
                .with_context(|| format!("移动失败：{} → {}", from.display(), to.display()))?;
        }
        self.buffer_store.rename_path(&indexed_from, &indexed_to);
        // from 不可能是根（校验已排除），条目变化无需区分 RootChanged。
        cx.emit(ProjectEvent::EntriesChanged);
        Ok(())
    }

    /// 在项目内递归复制文件或目录到新位置（后台执行，不阻塞 UI 线程）。
    ///
    /// 同步完成参数校验后返回驱动任务：复制本体在后台线程执行，完成后由任务内部发出 `EntriesChanged`；
    /// 任务返回值携带执行结果（调用方驱动进度用）。
    /// 失败详情已在此层输出日志（条目未变化时不发出事件），调用方可静默跳过失败项。
    pub fn copy_path(
        &mut self,
        source: &Path,
        destination: &Path,
        overwrite: bool,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Task<anyhow::Result<()>>> {
        let root = self
            .root()
            .ok_or_else(|| anyhow::anyhow!("当前项目没有 worktree"))?
            .to_path_buf();
        anyhow::ensure!(source != root, "不能复制项目根目录");
        anyhow::ensure!(source.starts_with(&root), "条目不在当前项目中");
        anyhow::ensure!(destination.starts_with(&root), "目标不在当前项目中");
        anyhow::ensure!(!destination.starts_with(source), "不能把条目复制到自身内部");
        // 对称守卫：目标是源的祖先目录时，覆盖路径的「先删目标」会把源一起递归删掉。
        anyhow::ensure!(
            !source.starts_with(destination),
            "不能把条目复制到自身的祖先目录：{}",
            destination.display()
        );
        anyhow::ensure!(
            overwrite || !destination.exists(),
            "目标已存在：{}",
            destination.display()
        );
        anyhow::ensure!(source.exists(), "源条目不存在：{}", source.display());
        // 不在同步阶段预删已存在目标：后台复制先把完整副本落到同级临时条目，成功后才替换目标（见 `copy_entry_overwrite`），任何一步失败原目标内容完好。
        let source = source.to_path_buf();
        let destination = destination.to_path_buf();
        // 任务交由调用方驱动（drop 即取消）：进度面板逐项 await 推进，不随 Project 持久保存字段。
        Ok(
            cx.spawn(move |project: WeakEntity<Self>, asynccx: &mut AsyncApp| {
                let mut cx = asynccx.clone();
                async move {
                    let result = cx
                        .background_executor()
                        .spawn(async move { copy_entry_overwrite(&source, &destination) })
                        .await;
                    match result {
                        Ok(()) => {
                            let _ = project.update(&mut cx, |_, cx| {
                                cx.emit(ProjectEvent::EntriesChanged);
                            });
                            Ok(())
                        }
                        Err(error) => {
                            eprintln!("项目复制失败：{error}");
                            Err(error)
                        }
                    }
                }
            }),
        )
    }

    fn process_fs_events(&mut self, events: Vec<PathEvent>, cx: &mut Context<Self>) {
        let Some(worktree) = &self.worktree else {
            return;
        };
        let events: Vec<_> = events
            .into_iter()
            .filter(|event| event.path.starts_with(&worktree.root))
            .collect();
        if events.is_empty() {
            return;
        }

        for event in &events {
            if matches!(
                event.kind,
                Some(PathEventKind::Changed | PathEventKind::Created)
            ) {
                self.buffer_store.reload_buffer_for_path(&event.path, cx);
            }
        }

        // git 状态刷新：删除/失步走全量扫描（涉及条目消失），文件变化走增量。
        // `.git/` 内只放行影响 git 状态的路径（HEAD/refs/index/packed-refs）：
        // 保住外部 checkout 兜底（HEAD/refs 变化触发 head 重读），砍掉 git 操作期间的对象/日志噪声风暴。
        let structural = events.iter().any(|event| {
            matches!(
                event.kind,
                Some(PathEventKind::Removed | PathEventKind::Rescan)
            )
        });
        let changed: Vec<PathBuf> = events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    Some(PathEventKind::Changed | PathEventKind::Created)
                )
            })
            .map(|event| event.path.clone())
            .filter(|path| keep_git_state_event(path))
            .collect();
        self.git_store.update(cx, |store, cx| {
            if structural {
                store.schedule_scan(cx);
            } else if !changed.is_empty() {
                store.refresh_statuses_for_paths(&changed, cx);
            }
        });

        cx.emit(ProjectEvent::EntriesChanged);
    }

    /// 查询文件的 git 状态（不在任何仓库或未跟踪时对应状态）。
    fn git_status_for_path(&self, path: &Path, cx: &App) -> Option<StatusEntry> {
        self.git_store.read(cx).status_for_path(path).cloned()
    }

    /// 查询目录的聚合 git 状态（子项中优先级最高的状态）。
    fn git_status_for_directory(&self, path: &Path, cx: &App) -> Option<FileStatus> {
        self.git_store.read(cx).status_for_directory(path)
    }
}

impl EventEmitter<ProjectEvent> for Project {}

/// `.git` 内路径只放行影响 git 状态的（HEAD/refs/index/packed-refs），其余丢弃。
///
/// git fetch/pull/push 期间 `.git` 下有大量对象/日志写入，全量进入增量 job 会触发无谓的 git 进程风暴；
/// HEAD/refs 变化仍放行，外部 checkout 的兜底语义不丢。
fn keep_git_state_event(path: &Path) -> bool {
    let mut components = path.components();
    while let Some(component) = components.next() {
        if component.as_os_str() == ".git" {
            let rest = components.as_path();
            return rest == Path::new("HEAD")
                || rest.starts_with("refs")
                || rest == Path::new("index")
                || rest == Path::new("packed-refs");
        }
    }
    // 非 .git 内路径一律放行。
    true
}

fn write_buffer_to_path(buffer: &mut Buffer, path: &Path) -> Result<(), BufferSaveError> {
    let version = buffer.version();
    let mut file = File::create(path)?;
    buffer.write_to(version, &mut file)?;
    file.sync_all()?;
    buffer.mark_saved();
    Ok(())
}

/// 删除已存在的文件或目录（移动/复制以 overwrite 替换目标时调用）。
///
/// 用 `symlink_metadata` 判断类型：符号链接删除链接本身（`remove_dir_all` 对链接会失败）。
fn remove_entry(path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// 生成目标的同级临时路径：同父目录，名称追加 `.zcv-copy-tmp` 后缀。
fn sibling_tmp_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .map(|name| format!("{}.zcv-copy-tmp", name.to_string_lossy()))
        .unwrap_or_else(|| ".zcv-copy-tmp".to_string());
    destination.with_file_name(name)
}

/// 递归复制并替换已存在目标：完整副本先落到目标的同级临时条目，成功后再 rename 入位。
///
/// 触碰已存在目标的唯一时机是复制完全成功之后（目录：删旧目标再 rename；
/// 文件：rename 原地替换），任何一步失败都清理临时产物并把错误传出，原目标内容不会被损坏。
fn copy_entry_overwrite(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let tmp = sibling_tmp_path(destination);
    let result = copy_entry_overwrite_inner(source, destination, &tmp);
    if result.is_err() && tmp.symlink_metadata().is_ok() {
        // 失败清理：临时产物不残留（失败路径未触碰原目标，无需恢复）。
        let _ = remove_entry(&tmp);
    }
    result
}

fn copy_entry_overwrite_inner(source: &Path, destination: &Path, tmp: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("读取源条目失败：{}", source.display()))?;
    if metadata.file_type().is_dir() {
        // 目录：整树复制到临时目录 → 删除旧目标 → rename 入位。
        copy_entry_recursive(source, tmp)?;
        if destination.exists() {
            remove_entry(destination)?;
        }
        std::fs::rename(tmp, destination)
            .with_context(|| format!("临时目录入位失败：{}", destination.display()))?;
    } else {
        // 文件与符号链接：先写临时文件再 rename 入位（POSIX rename 原子替换文件目标）。
        copy_single_entry(source, tmp, metadata.file_type())?;
        // 已存在目标类型不匹配（如文件覆盖目录）时 rename 会失败：先行删除。
        if destination.exists() && !destination.is_file() {
            remove_entry(destination)?;
        }
        if std::fs::rename(tmp, destination).is_err() {
            // 兜底：个别平台 rename 不自动替换已存在文件目标，删除后重试一次。
            if destination.exists() {
                remove_entry(destination)?;
            }
            std::fs::rename(tmp, destination)
                .with_context(|| format!("临时文件入位失败：{}", destination.display()))?;
        }
    }
    Ok(())
}

/// 复制单个非目录条目（文件或符号链接）到目标路径。
///
/// 符号链接按链接本身复制（读出链接目标后重建），不跟随链接指向的内容。
fn copy_single_entry(
    source: &Path,
    destination: &Path,
    file_type: std::fs::FileType,
) -> anyhow::Result<()> {
    if file_type.is_symlink() {
        let target = std::fs::read_link(source)
            .with_context(|| format!("读取符号链接失败：{}", source.display()))?;
        std::os::unix::fs::symlink(&target, destination)
            .with_context(|| format!("重建符号链接失败：{}", destination.display()))?;
    } else {
        std::fs::copy(source, destination)
            .with_context(|| format!("复制文件失败：{}", source.display()))?;
    }
    Ok(())
}

/// 递归复制文件或目录（同步实现，供后台线程调用）。
///
/// 类型判断基于 `symlink_metadata`：符号链接按链接本身复制，不跟随链接目标，避免指向祖先目录的链接环引发无限递归。
fn copy_entry_recursive(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("读取源条目失败：{}", source.display()))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        copy_single_entry(source, destination, file_type)?;
    } else if file_type.is_dir() {
        std::fs::create_dir_all(destination)
            .with_context(|| format!("创建目录失败：{}", destination.display()))?;
        for entry in std::fs::read_dir(source)
            .with_context(|| format!("读取目录失败：{}", source.display()))?
        {
            let entry = entry?;
            copy_entry_recursive(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(source, destination)
            .with_context(|| format!("复制文件失败：{}", source.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use gpui::{AppContext, TestAppContext};
    use zcv_text::{BufferConfig, ByteOffset, Edit, TransactionMetadata};

    use super::*;
    use crate::test_support::test_git_repo;

    #[gpui::test]
    fn empty_project_has_no_worktree_or_project_services(cx: &mut TestAppContext) {
        let project = cx.update(|cx| cx.new(Project::empty));
        cx.read_entity(&project, |project, _| {
            assert!(!project.has_worktree());
            assert!(project.root().is_none());
            assert!(project.try_git_store().is_none());
        });
    }

    #[test]
    fn saving_buffer_writes_current_version_and_marks_it_clean() {
        let path = test_file_path();
        let mut buffer =
            Buffer::scratch("旧内容".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
        buffer
            .edit(
                [Edit::insert(buffer.len_bytes(), " + 新内容").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");
        assert!(buffer.is_dirty());

        write_buffer_to_path(&mut buffer, &path).expect("保存应成功");

        assert_eq!(
            fs::read_to_string(&path).expect("应读回文件"),
            "旧内容 + 新内容"
        );
        assert!(!buffer.is_dirty());
        fs::remove_file(path).expect("测试文件应可删除");
    }

    #[test]
    fn failed_save_keeps_buffer_dirty() {
        let path = test_file_path().join("missing.txt");
        let mut buffer =
            Buffer::scratch("内容".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
        buffer
            .edit(
                [Edit::insert(ByteOffset::ZERO, "未保存").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");

        assert!(write_buffer_to_path(&mut buffer, &path).is_err());
        assert!(buffer.is_dirty());
    }

    #[gpui::test]
    fn renaming_file_keeps_open_buffer_indexed_by_new_path(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let old_path = directory.path().join("old.txt");
        let new_path = directory.path().join("new.txt");
        fs::write(&old_path, "content").expect("应创建测试文件");

        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
        let original = project.update(cx, |project, cx| {
            project.open_buffer(&old_path, cx).expect("应打开测试文件")
        });
        project
            .update(cx, |project, cx| {
                project.rename_path(&old_path, &new_path, cx)
            })
            .expect("应重命名测试文件");
        let reopened = project.update(cx, |project, cx| {
            project
                .open_buffer(&new_path, cx)
                .expect("应从新路径打开文件")
        });

        assert_eq!(original, reopened);
        assert!(!old_path.exists());
        assert!(new_path.exists());
    }

    #[gpui::test]
    fn creating_path_rejects_existing_file_and_directory_collisions(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let file = directory.path().join("src/components/new.txt");
        let folder = directory.path().join("assets/icons/new-folder");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        project
            .update(cx, |project, cx| project.create_path(&file, false, cx))
            .expect("应创建文件");
        project
            .update(cx, |project, cx| project.create_path(&folder, true, cx))
            .expect("应创建目录");
        fs::write(&file, "existing content").expect("应写入已有文件内容");

        for (path, is_dir) in [
            (&file, false),
            (&file, true),
            (&folder, false),
            (&folder, true),
        ] {
            assert!(
                project
                    .update(cx, |project, cx| project.create_path(path, is_dir, cx))
                    .is_err(),
                "不应覆盖已有条目：{}",
                path.display()
            );
        }
        assert_eq!(
            fs::read_to_string(&file).expect("应读取已有文件"),
            "existing content",
            "创建冲突不应改动已有文件内容"
        );
        assert!(folder.is_dir(), "创建冲突不应替换已有目录");

        let unsafe_path = directory.path().join("../outside.txt");
        assert!(
            project
                .update(cx, |project, cx| {
                    project.create_path(&unsafe_path, false, cx)
                })
                .is_err(),
            "不应允许父目录逃逸"
        );
    }

    #[gpui::test]
    fn trashing_path_rejects_project_root_and_outside_entries(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        for path in [
            directory.path().to_path_buf(),
            PathBuf::from("/outside/file.txt"),
        ] {
            assert!(
                project
                    .update(cx, |project, cx| project.trash_path(&path, cx))
                    .is_err(),
                "不应允许删除 {}",
                path.display()
            );
        }
    }

    #[gpui::test]
    fn trashing_path_moves_file_to_system_trash(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let file = directory.path().join("to-trash.txt");
        fs::write(&file, "content").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        project.update(cx, |project, cx| {
            project.trash_path(&file, cx).expect("应移到系统废纸篓")
        });

        assert!(!file.exists(), "被删除文件应不再位于原路径");
    }

    #[gpui::test]
    fn moving_file_across_directories_keeps_open_buffer_indexed_by_new_path(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let old_path = directory.path().join("old.txt");
        let new_path = directory.path().join("sub").join("new.txt");
        fs::create_dir(directory.path().join("sub")).expect("应创建子目录");
        fs::write(&old_path, "content").expect("应创建测试文件");

        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
        let original = project.update(cx, |project, cx| {
            project.open_buffer(&old_path, cx).expect("应打开测试文件")
        });
        project
            .update(cx, |project, cx| {
                project.move_path(&old_path, &new_path, false, cx)
            })
            .expect("应跨目录移动测试文件");
        let reopened = project.update(cx, |project, cx| {
            project
                .open_buffer(&new_path, cx)
                .expect("应从新路径打开文件")
        });

        assert_eq!(original, reopened);
        assert!(!old_path.exists());
        assert!(new_path.exists());
    }

    #[gpui::test]
    fn moving_directory_into_itself_is_rejected(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let dir = directory.path().join("dir");
        fs::create_dir_all(dir.join("sub")).expect("应创建嵌套目录");
        fs::write(dir.join("file.txt"), "内容").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        let destination = dir.join("sub").join("x");
        assert!(
            project
                .update(cx, |project, cx| {
                    project.move_path(&dir, &destination, false, cx)
                })
                .is_err(),
            "不应允许把目录移动到自身内部"
        );
        assert!(dir.is_dir(), "原目录应完好");
        assert!(dir.join("file.txt").is_file(), "原目录内文件应完好");
        assert!(!destination.exists(), "目标不应被创建");
    }

    #[gpui::test]
    fn moving_file_overwrites_or_rejects_existing_destination(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let source = directory.path().join("source.txt");
        let target = directory.path().join("target.txt");
        fs::write(&source, "源内容").expect("应创建源文件");
        fs::write(&target, "目标内容").expect("应创建目标文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        assert!(
            project
                .update(cx, |project, cx| {
                    project.move_path(&source, &target, false, cx)
                })
                .is_err(),
            "无 overwrite 时冲突应被拒绝"
        );
        assert_eq!(
            fs::read_to_string(&target).expect("应读取目标文件"),
            "目标内容",
            "冲突被拒后目标内容不应变化"
        );

        project
            .update(cx, |project, cx| {
                project.move_path(&source, &target, true, cx)
            })
            .expect("overwrite 时应替换目标文件");
        assert_eq!(
            fs::read_to_string(&target).expect("应读取目标文件"),
            "源内容",
            "替换后目标内容应为源内容"
        );
        assert!(!source.exists(), "源文件应已移走");
    }

    #[gpui::test]
    fn moving_directory_moves_nested_files(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let source_dir = directory.path().join("src");
        let target_dir = directory.path().join("dest");
        fs::create_dir_all(source_dir.join("nested")).expect("应创建嵌套目录");
        fs::write(source_dir.join("nested").join("file.txt"), "内容").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        project
            .update(cx, |project, cx| {
                project.move_path(&source_dir, &target_dir, false, cx)
            })
            .expect("应移动目录");

        assert!(!source_dir.exists(), "旧目录不应再存在");
        assert_eq!(
            fs::read_to_string(target_dir.join("nested").join("file.txt"))
                .expect("应读取迁移后的文件"),
            "内容"
        );
    }

    #[gpui::test]
    fn moving_directory_with_overwrite_replaces_existing_directory(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let source_dir = directory.path().join("src");
        let target_dir = directory.path().join("dest");
        fs::create_dir_all(source_dir.join("nested")).expect("应创建源目录");
        fs::write(source_dir.join("nested").join("file.txt"), "新内容").expect("应创建测试文件");
        fs::create_dir_all(target_dir.join("old")).expect("应创建目标目录");
        fs::write(target_dir.join("old").join("legacy.txt"), "旧内容").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        project
            .update(cx, |project, cx| {
                project.move_path(&source_dir, &target_dir, true, cx)
            })
            .expect("overwrite 时应替换目标目录");

        assert!(!source_dir.exists(), "源目录应已移走");
        assert_eq!(
            fs::read_to_string(target_dir.join("nested").join("file.txt"))
                .expect("应读取替换后的文件"),
            "新内容"
        );
        assert!(
            !target_dir.join("old").join("legacy.txt").exists(),
            "目标目录原有内容应被移除"
        );
    }

    #[gpui::test]
    async fn copying_directory_recursively_replicates_contents(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let source_dir = directory.path().join("src");
        fs::create_dir_all(source_dir.join("嵌套")).expect("应创建嵌套目录");
        fs::write(source_dir.join("顶层.md"), "顶层内容").expect("应创建测试文件");
        fs::write(source_dir.join("嵌套").join("中文文件.txt"), "嵌套内容")
            .expect("应创建测试文件");
        let destination_dir = directory.path().join("copy");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        let task = project
            .update(cx, |project, cx| {
                project.copy_path(&source_dir, &destination_dir, false, cx)
            })
            .expect("应复制目录");
        // 复制本体在后台线程执行，await 任务后新路径才存在。
        task.await.expect("复制任务应成功");

        assert_eq!(
            fs::read_to_string(destination_dir.join("顶层.md")).expect("应读取复制出的文件"),
            "顶层内容"
        );
        assert_eq!(
            fs::read_to_string(destination_dir.join("嵌套").join("中文文件.txt"))
                .expect("应读取复制出的文件"),
            "嵌套内容"
        );
        // 复制不改动源目录。
        assert!(source_dir.join("嵌套").join("中文文件.txt").is_file());
    }

    #[gpui::test]
    fn copying_into_own_subtree_is_rejected(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let source_dir = directory.path().join("src");
        fs::create_dir_all(&source_dir).expect("应创建源目录");
        fs::write(source_dir.join("file.txt"), "内容").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        let destination = source_dir.join("copy");
        assert!(
            project
                .update(cx, |project, cx| {
                    project.copy_path(&source_dir, &destination, false, cx)
                })
                .is_err(),
            "不应允许把目录复制到自身内部"
        );
        assert!(!destination.exists(), "目标不应被创建");
    }

    #[gpui::test]
    fn copying_without_overwrite_rejects_existing_destination(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let source = directory.path().join("source.txt");
        let destination = directory.path().join("destination.txt");
        fs::write(&source, "源内容").expect("应创建源文件");
        fs::write(&destination, "目标内容").expect("应创建目标文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        assert!(
            project
                .update(cx, |project, cx| {
                    project.copy_path(&source, &destination, false, cx)
                })
                .is_err(),
            "无 overwrite 时冲突应被拒绝"
        );
        assert_eq!(
            fs::read_to_string(&destination).expect("应读取目标文件"),
            "目标内容",
            "冲突被拒后目标内容不应变化"
        );
        assert!(source.is_file(), "源文件不应被删除");
    }

    #[gpui::test]
    async fn copying_file_completes_in_background(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let source = directory.path().join("source.txt");
        let destination = directory.path().join("destination.txt");
        fs::write(&source, "内容").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        let task = project
            .update(cx, |project, cx| {
                project.copy_path(&source, &destination, false, cx)
            })
            .expect("应复制文件");
        // 复制本体在后台线程执行，await 任务后新路径才存在。
        task.await.expect("复制任务应成功");

        assert_eq!(
            fs::read_to_string(&destination).expect("应读取复制出的文件"),
            "内容"
        );
        assert!(source.is_file(), "复制不删除源文件");
    }

    #[gpui::test]
    async fn copying_with_overwrite_replaces_existing_destination(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let source = directory.path().join("source.txt");
        let destination = directory.path().join("destination.txt");
        fs::write(&source, "新内容").expect("应创建源文件");
        fs::write(&destination, "旧内容").expect("应创建目标文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        let task = project
            .update(cx, |project, cx| {
                project.copy_path(&source, &destination, true, cx)
            })
            .expect("overwrite 时应替换目标文件");
        task.await.expect("复制任务应成功");

        assert_eq!(
            fs::read_to_string(&destination).expect("应读取目标文件"),
            "新内容",
            "复制后目标内容应为源内容"
        );
        assert!(source.is_file(), "复制不删除源文件");
    }

    #[gpui::test]
    fn moving_directory_into_own_ancestor_is_rejected_even_with_overwrite(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let ancestor = directory.path().join("dir");
        let source = ancestor.join("sub");
        fs::create_dir_all(&source).expect("应创建源目录");
        fs::write(source.join("file.txt"), "源内容").expect("应创建测试文件");
        fs::write(ancestor.join("keep.txt"), "原有内容").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        // 目标是源的祖先目录；若不拦截，覆盖路径的「先删目标」会把源一起递归删掉。
        let result = project.update(cx, |project, cx| {
            project.move_path(&source, &ancestor, true, cx)
        });
        assert!(result.is_err(), "不应允许把条目移动到自身的祖先目录");
        assert!(source.is_dir(), "源目录应完好");
        assert_eq!(
            fs::read_to_string(source.join("file.txt")).expect("应读取源目录内文件"),
            "源内容",
            "源目录内容不应被破坏"
        );
        assert_eq!(
            fs::read_to_string(ancestor.join("keep.txt")).expect("应读取祖先目录原有文件"),
            "原有内容",
            "祖先目录原有内容不应被破坏"
        );
    }

    #[gpui::test]
    fn copying_directory_into_own_ancestor_is_rejected(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let source = directory.path().join("src");
        fs::create_dir_all(&source).expect("应创建源目录");
        fs::write(source.join("file.txt"), "内容").expect("应创建测试文件");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));

        // 目标是项目根（源的祖先）：即便允许覆盖也必须拒绝。
        let destination = directory.path().to_path_buf();
        let result = project.update(cx, |project, cx| {
            project.copy_path(&source, &destination, true, cx)
        });
        assert!(result.is_err(), "不应允许把条目复制到自身的祖先目录");
        assert!(source.join("file.txt").is_file(), "源目录内容应完好");
    }

    #[gpui::test]
    fn fs_events_trigger_incremental_git_status_refresh(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let project = cx.new(|cx| Project::new(root.clone(), cx));
        cx.run_until_parked();

        // 初始扫描后文件干净，无 git 状态。
        let file = root.join("tracked.txt");
        assert!(
            project
                .update(cx, |project, cx| project.git_status_for_path(&file, cx))
                .is_none()
        );

        // 文件被外部修改 → fs 事件 → 增量刷新。
        fs::write(&file, "已修改\n").expect("应修改文件");
        project.update(cx, |project, cx| {
            project.process_fs_events(
                vec![PathEvent {
                    path: file.clone(),
                    kind: Some(PathEventKind::Changed),
                }],
                cx,
            );
        });
        cx.run_until_parked();

        let entry = project
            .update(cx, |project, cx| project.git_status_for_path(&file, cx))
            .expect("应有 git 状态");
        assert!(entry.status.is_modified());
    }

    #[gpui::test]
    fn fs_removal_events_trigger_full_rescan(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let project = cx.new(|cx| Project::new(root.clone(), cx));
        cx.run_until_parked();

        // 未跟踪文件出现，随后被删除：Removed 事件应触发全量扫描，
        // 状态表不再包含该路径。
        let file = root.join("scratch.txt");
        fs::write(&file, "临时\n").expect("应创建文件");
        project.update(cx, |project, cx| {
            project.process_fs_events(
                vec![PathEvent {
                    path: file.clone(),
                    kind: Some(PathEventKind::Created),
                }],
                cx,
            );
        });
        cx.run_until_parked();
        assert!(
            project
                .update(cx, |project, cx| project.git_status_for_path(&file, cx))
                .is_some()
        );

        fs::remove_file(&file).expect("应删除文件");
        project.update(cx, |project, cx| {
            project.process_fs_events(
                vec![PathEvent {
                    path: file.clone(),
                    kind: Some(PathEventKind::Removed),
                }],
                cx,
            );
        });
        cx.run_until_parked();
        assert!(
            project
                .update(cx, |project, cx| project.git_status_for_path(&file, cx))
                .is_none()
        );
    }

    // 依赖真实 FSEvents 事件：并行测试下系统会合并/延迟事件导致偶发超时，
    // 串行（--test-threads=1）或单独运行时稳定。用 `cargo test -- --ignored` 显式验证。
    #[gpui::test]
    #[ignore]
    fn real_fs_watcher_triggers_git_refresh(cx: &mut gpui::TestAppContext) {
        // 模拟生产的 Project root：生产路径经 canonicalize 归一化（macOS 上
        // /var → /private/var），否则 FSEvents 返回的实际路径与注册路径
        // 前缀不匹配，事件会被 fs_watcher 过滤掉。
        let (root, _temp) = test_git_repo();
        let root = root.canonicalize().expect("应可 canonicalize");
        let project = cx.new(|cx| Project::new(root.clone(), cx));
        cx.run_until_parked();

        // 等 notify 在后台线程建立 watch，避免写入事件丢失。
        std::thread::sleep(std::time::Duration::from_millis(500));
        // 真实写文件 → notify 监听 → process_fs_events → git 增量刷新。
        fs::write(root.join("tracked.txt"), "外部修改\n").expect("应写入文件");
        let file = root.join("tracked.txt");
        // FSEvents 事件在并行测试负载下可能延迟数秒，放宽超时。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            cx.run_until_parked();
            if project
                .update(cx, |project, cx| project.git_status_for_path(&file, cx))
                .is_some()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "等待 fs 事件驱动的 git 刷新超时"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[gpui::test]
    fn saving_buffer_refreshes_git_status(cx: &mut gpui::TestAppContext) {
        let (root, _temp) = test_git_repo();
        let project = cx.new(|cx| Project::new(root.clone(), cx));
        cx.run_until_parked();

        // 打开并修改 buffer（未保存），git 状态应仍为干净（status 反映磁盘）。
        let file = root.join("tracked.txt");
        let buffer = project
            .update(cx, |project, cx| project.open_buffer(&file, cx))
            .expect("应打开文件");
        let engine_buffer = cx.read_entity(&buffer, |multi_buffer, cx| {
            multi_buffer
                .as_singleton(cx)
                .expect("测试文档应是 singleton")
        });
        engine_buffer
            .update(cx, |buffer, _| {
                buffer.edit(
                    [Edit::insert(buffer.len_bytes(), "新增行\n").unwrap()],
                    TransactionMetadata::default(),
                )
            })
            .expect("编辑应成功");
        cx.run_until_parked();
        assert!(
            project
                .update(cx, |project, cx| project.git_status_for_path(&file, cx))
                .is_none()
        );

        // 保存后 git 状态应变为已修改。
        project
            .update(cx, |project, cx| project.save_buffer(&buffer, &file, cx))
            .expect("保存应成功");
        cx.run_until_parked();
        let entry = project
            .update(cx, |project, cx| project.git_status_for_path(&file, cx))
            .expect("保存后应有 git 状态");
        assert!(entry.status.is_modified());
    }

    /// 复制失败的注入点选在同步入口（源不存在）：直接调内部函数验证失败路径。
    /// （copy_path 后台任务的失败同样从 `copy_entry_overwrite` 起源，覆盖同一条失败链。）
    #[test]
    fn failed_copy_preserves_existing_destination_and_leaves_no_tmp() {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let source = directory.path().join("missing.txt");
        let destination = directory.path().join("destination.txt");
        fs::write(&destination, "目标内容").expect("应创建目标文件");

        assert!(
            copy_entry_overwrite(&source, &destination).is_err(),
            "源不存在时复制应失败"
        );
        assert_eq!(
            fs::read_to_string(&destination).expect("应读取目标文件"),
            "目标内容",
            "复制失败不应破坏原目标"
        );
        assert!(
            !sibling_tmp_path(&destination).exists(),
            "失败后不应残留临时文件"
        );
    }

    #[test]
    fn overwrite_copy_replaces_existing_file_and_directory() {
        // 文件覆盖文件：目标内容被替换，源不动。
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let source_file = directory.path().join("source.txt");
        let dest_file = directory.path().join("destination.txt");
        fs::write(&source_file, "新内容").expect("应创建源文件");
        fs::write(&dest_file, "旧内容").expect("应创建目标文件");
        copy_entry_overwrite(&source_file, &dest_file).expect("应覆盖文件");
        assert_eq!(
            fs::read_to_string(&dest_file).expect("应读取目标文件"),
            "新内容"
        );

        // 目录覆盖目录：旧内容被移除，新内容入位。
        let source_dir = directory.path().join("source-dir");
        let dest_dir = directory.path().join("destination-dir");
        fs::create_dir_all(&source_dir).expect("应创建源目录");
        fs::write(source_dir.join("new.txt"), "新目录内容").expect("应创建测试文件");
        fs::create_dir_all(&dest_dir).expect("应创建目标目录");
        fs::write(dest_dir.join("old.txt"), "旧目录内容").expect("应创建测试文件");
        copy_entry_overwrite(&source_dir, &dest_dir).expect("应覆盖目录");
        assert_eq!(
            fs::read_to_string(dest_dir.join("new.txt")).expect("应读取替换后的文件"),
            "新目录内容"
        );
        assert!(!dest_dir.join("old.txt").exists(), "目标目录旧内容应被移除");
        assert!(source_dir.join("new.txt").is_file(), "源目录不应被删除");
    }

    #[test]
    fn copying_directory_with_ancestor_symlink_does_not_hang() {
        // 链接指向自身祖先目录（链接环）：按链接本身复制，不跟随目标、不挂死。
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let source_dir = directory.path().join("dir");
        fs::create_dir_all(&source_dir).expect("应创建源目录");
        fs::write(source_dir.join("real.txt"), "内容").expect("应创建测试文件");
        std::os::unix::fs::symlink(directory.path(), source_dir.join("loop"))
            .expect("应创建指向祖先目录的符号链接");
        let destination = directory.path().join("copy");

        copy_entry_recursive(&source_dir, &destination).expect("应完成含链接环的复制");

        assert_eq!(
            fs::read_to_string(destination.join("real.txt")).expect("应读取复制出的文件"),
            "内容"
        );
        let link = destination.join("loop");
        assert!(
            link.symlink_metadata()
                .expect("应读取链接元信息")
                .file_type()
                .is_symlink(),
            "链接应按链接本身复制"
        );
        assert_eq!(
            fs::read_link(&link).expect("应读取链接目标"),
            directory.path().to_path_buf(),
            "链接目标应保持原样"
        );
    }

    fn test_file_path() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应晚于 Unix Epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "project-save-test-{}-{nonce}.txt",
            std::process::id()
        ))
    }
}
