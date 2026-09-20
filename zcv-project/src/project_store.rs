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
use zcv_git::{ConflictChoice, FileStatus, parse_conflict_regions, resolve_conflict};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_path::{AbsolutePathBuf, normalize_for_comparison};
use zcv_text::{Buffer, ByteOffset, Edit, Snapshot, TextRange, TransactionMetadata};

use crate::search::SearchQuery;

mod platform;

use super::buffer_store::BufferStore;
use super::git_store::{GitStatusSnapshot, GitStore};
use super::search::{self, SearchResults};
use super::text_file::{BufferLoadError, BufferSaveError, LineEndingConfig, write_buffer_to};
use super::worktree::{Worktree, WorktreeEntry, collect_visible_entries};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileWatcherOperation {
    Add,
    Remove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileWatcherError {
    pub operation: FileWatcherOperation,
    pub path: PathBuf,
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectEvent {
    RootChanged(PathBuf),
    EntriesChanged,
    FileWatcherError(FileWatcherError),
}

pub struct Project {
    /// Project 始终存在，worktree 可以为空。
    worktree: Option<ProjectWorktree>,
    /// git store 属于 Project 而非 worktree，无 worktree 时以无根状态存在（仓库查询与 git job 为空操作）。
    git_store: Entity<GitStore>,
    buffer_store: BufferStore,
    /// 应用装配层创建并注入的唯一语言注册表；项目内所有语言 Buffer 与 diff 源共享同一份，避免多处独立加载。
    language_registry: Arc<LanguageRegistry>,
    /// 项目创建阶段尚未建立工作区订阅时产生的监听错误。
    pending_file_watcher_errors: Vec<FileWatcherError>,
}

struct ProjectWorktree {
    root: AbsolutePathBuf,
    snapshot: Worktree,
    fs_watcher: Arc<dyn Watcher>,
    _fs_task: Task<()>,
}

impl Project {
    /// 使用默认文件监听后端创建项目。
    ///
    /// 语言注册表由应用装配层创建并显式注入，项目只持有同一份 Arc，不在内部新建。
    pub fn new(root: PathBuf, languages: Arc<LanguageRegistry>, cx: &mut Context<Self>) -> Self {
        Self::new_with_watcher(root, Arc::new(FsWatcher::new()), languages, cx)
    }

    /// 使用指定的文件监听后端创建项目。
    ///
    /// 项目负责监听器的生命周期和事件消费；调用方负责选择符合当前运行环境的后端。
    /// 语言注册表由应用装配层创建并显式注入，项目内的语言 Buffer 与 diff 源共享同一份。
    pub fn new_with_watcher(
        root: PathBuf,
        fs_watcher: Arc<dyn Watcher>,
        languages: Arc<LanguageRegistry>,
        cx: &mut Context<Self>,
    ) -> Self {
        // ProjectWorktree、文件监听器和 GitStore 共用同一个已规范化根路径。
        // Project 只能由已确认存在的目录构造，失败应在装配阶段暴露，而不是在各消费者中分别回退。
        let root =
            AbsolutePathBuf::canonicalize(&root).expect("Project 根目录必须是可规范化的已存在目录");
        let fs_events = fs_watcher.events();

        let pending_file_watcher_errors = match fs_watcher.add(root.as_path()) {
            Ok(()) => Vec::new(),
            Err(error) => vec![FileWatcherError {
                operation: FileWatcherOperation::Add,
                path: root.as_path().to_path_buf(),
                error: format!("{error:#}"),
            }],
        };

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

        let git_store = cx.new(|cx| {
            GitStore::new(
                Some(root.as_path().to_path_buf()),
                Arc::clone(&languages),
                cx,
            )
        });
        git_store.update(cx, |store, cx| store.schedule_scan(cx));

        Self {
            worktree: Some(ProjectWorktree {
                root: root.clone(),
                snapshot: Worktree::new(root.clone()),
                fs_watcher,
                _fs_task: fs_task,
            }),
            git_store,
            buffer_store: BufferStore::new(Arc::clone(&languages)),
            language_registry: languages,
            pending_file_watcher_errors,
        }
    }

    /// 创建没有 worktree 的本地项目，供空工作区使用。
    ///
    /// 语言注册表同样由应用装配层注入。
    pub fn empty(languages: Arc<LanguageRegistry>, cx: &mut Context<Self>) -> Self {
        let git_store = cx.new(|cx| GitStore::new(None, Arc::clone(&languages), cx));
        Self {
            worktree: None,
            git_store,
            buffer_store: BufferStore::new(Arc::clone(&languages)),
            language_registry: languages,
            pending_file_watcher_errors: Vec::new(),
        }
    }

    /// 项目唯一的语言注册表。
    pub fn language_registry(&self) -> Arc<LanguageRegistry> {
        Arc::clone(&self.language_registry)
    }

    /// 取出项目创建期间尚未通过 ProjectEvent 投递的文件监听错误。
    pub fn take_pending_file_watcher_errors(&mut self) -> Vec<FileWatcherError> {
        std::mem::take(&mut self.pending_file_watcher_errors)
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

    /// 后台收集当前展开状态下的可见行，返回后台任务。
    ///
    /// Git 状态由项目树在行模型应用后单独批量查询，避免目录扫描与状态快照之间形成竞态，也让两类派生数据拥有各自明确的失效边界。
    pub fn collect_visible_rows(
        &self,
        expanded: HashSet<AbsolutePathBuf>,
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

    /// 后台批量查询可见行的 Git 状态（Git 事件驱动，不重扫目录）。
    ///
    /// `rows` 为 (路径, 是否目录) 对：目录行取聚合状态，文件行取精确状态。
    pub fn git_statuses_for_rows(
        &self,
        rows: Vec<(AbsolutePathBuf, bool)>,
        cx: &App,
    ) -> Task<HashMap<AbsolutePathBuf, FileStatus>> {
        let snapshot: Arc<GitStatusSnapshot> = self.git_store.read(cx).status_snapshot();
        cx.background_executor()
            .spawn(async move { snapshot.statuses_for_rows(&rows) })
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
    ) -> Result<Entity<LanguageBuffer>, BufferLoadError> {
        self.buffer_store.open_buffer(path, cx)
    }

    /// 解决工作区文件中的一个 Git 冲突，并把结果保存回文件。
    ///
    /// 冲突解析属于项目工作区事务：
    /// 调用方只提交路径、冲突序号和选择，Buffer 编辑、落盘及 Git 状态刷新由 Project 统一完成。
    pub fn resolve_conflict(
        &mut self,
        path: &Path,
        conflict_index: usize,
        choice: ConflictChoice,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let language_buffer = self.open_buffer(path, cx)?;
        let snapshot = language_buffer.read(cx).text_snapshot();
        let text_range = TextRange::new(ByteOffset::ZERO, snapshot.len_bytes())?;
        let text = snapshot.slice_text(text_range)?.to_string();
        let regions = parse_conflict_regions(&text);
        let region = regions
            .get(conflict_index)
            .ok_or_else(|| anyhow::anyhow!("冲突序号无效：{conflict_index}"))?;
        let resolved = resolve_conflict(&text, region, choice);
        let full_range = TextRange::new(ByteOffset::ZERO, ByteOffset::new(text.len()))?;
        language_buffer.update(cx, |language_buffer, cx| {
            language_buffer.edit(
                [Edit::replace(full_range, resolved)],
                TransactionMetadata::default(),
                cx,
            )
        })?;
        self.save_file_buffers(vec![(language_buffer, path.to_path_buf())], cx)?;
        Ok(())
    }

    /// 为 Git 删除状态打开空的工作区侧文档。
    ///
    /// 该入口只负责文件 Buffer 生命周期；
    /// HEAD 文本和 hunk 仍由 GitStore 持有。
    pub fn open_deleted_buffer(
        &mut self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Result<Entity<LanguageBuffer>, BufferLoadError> {
        self.buffer_store.open_deleted_buffer(path, cx)
    }

    /// 在后台逐文件扫描 worktree 并预加载命中文件，
    /// 结果经流式通道产出，由 UI 线程按批装配进 MultiBuffer。
    pub fn search(&mut self, query: SearchQuery, cx: &mut Context<Self>) -> SearchResults {
        let Some(worktree) = &self.worktree else {
            return SearchResults::empty();
        };
        let plan = worktree.snapshot.search_plan();
        let opened_snapshots = self.buffer_store.opened_snapshots(cx);
        let background_executor = cx.background_executor().clone();
        let language_registry = Arc::clone(&self.language_registry);
        let (tx, rx) = async_channel::bounded(8);
        let task = cx.background_executor().spawn(async move {
            let _ = search::search_worktree(
                plan,
                opened_snapshots,
                query,
                language_registry,
                tx,
                background_executor,
            )
            .await;
        });
        SearchResults { task, rx }
    }

    /// 注册搜索任务在后台加载完成的 Buffer，与已打开文档共享同一缓存。
    pub fn register_loaded_buffer(
        &mut self,
        path: PathBuf,
        buffer: Buffer,
        cx: &mut Context<Self>,
    ) -> Result<Entity<LanguageBuffer>, BufferLoadError> {
        self.buffer_store.register_loaded_buffer(path, buffer, cx)
    }

    /// 保存真实源文件的 Buffer；组合投影不会参与落盘。
    pub fn save_file_buffers(
        &mut self,
        buffers: Vec<(Entity<LanguageBuffer>, PathBuf)>,
        cx: &mut Context<Self>,
    ) -> Result<(), BufferSaveError> {
        let mut saved_paths = Vec::with_capacity(buffers.len());
        let mut resolved_conflict_paths: Vec<AbsolutePathBuf> = Vec::new();
        for (language_buffer, path) in buffers {
            let snapshot = language_buffer.read(cx).text_snapshot();
            let is_unmerged = self
                .git_store
                .read(cx)
                .status_for_path(&path)
                .is_some_and(|entry| entry.status == FileStatus::Unmerged);
            if is_unmerged {
                let range = TextRange::new(ByteOffset::ZERO, snapshot.len_bytes())
                    .expect("文本快照的全文范围必须有效");
                let text = snapshot
                    .slice_text(range)
                    .expect("文本快照必须可切片")
                    .to_string();
                if parse_conflict_regions(&text).is_empty() {
                    resolved_conflict_paths.push(
                        AbsolutePathBuf::canonicalize(&path)
                            .expect("已保存的冲突路径必须是绝对路径"),
                    );
                }
            }
            write_buffer_to_path(&snapshot, &path)?;
            language_buffer.update(cx, |language_buffer, cx| language_buffer.mark_saved(cx));
            saved_paths.push(path);
        }
        // 保存成功后立即刷新 git 状态（快路径，不等 fs 事件；
        // fs 事件晚到会被 job 去重吸收）。
        if !saved_paths.is_empty() {
            self.git_store.update(cx, |store, cx| {
                store.refresh_statuses_for_paths(&saved_paths, cx);
                if !resolved_conflict_paths.is_empty() {
                    store.resolve_conflicts(resolved_conflict_paths, cx);
                }
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
        let from = normalize_for_comparison(from)?;
        let to = normalize_for_comparison(to)?;
        anyhow::ensure!(from != to, "新旧路径不能相同");
        anyhow::ensure!(from.parent() == to.parent(), "重命名不能移动条目");
        let worktree = self
            .worktree
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("当前项目没有 worktree"))?;
        anyhow::ensure!(
            from.as_path() == worktree.root.as_path() || from.starts_with(worktree.root.as_path()),
            "条目不在当前项目中"
        );
        let indexed_from = AbsolutePathBuf::canonicalize(&from)?;
        if to.exists() {
            anyhow::ensure!(
                AbsolutePathBuf::canonicalize(&to)?.as_path() == indexed_from.as_path(),
                "目标已存在：{}",
                to.display()
            );
        }
        let indexed_to = indexed_from
            .parent()
            .and_then(|parent| to.file_name().map(|name| parent.join(name)))
            .ok_or_else(|| anyhow::anyhow!("无法确定重命名目标路径"))?;
        std::fs::rename(&from, &to)?;
        self.buffer_store
            .rename_path(indexed_from.as_path(), &indexed_to);

        if from.as_path() == worktree.root.as_path() {
            let new_root = AbsolutePathBuf::canonicalize(&to)?;
            if let Err(error) = worktree.fs_watcher.add(&to) {
                cx.emit(ProjectEvent::FileWatcherError(FileWatcherError {
                    operation: FileWatcherOperation::Add,
                    path: to.as_path().to_path_buf(),
                    error: format!("{error:#}"),
                }));
            }
            if let Err(error) = worktree.fs_watcher.remove(&from) {
                cx.emit(ProjectEvent::FileWatcherError(FileWatcherError {
                    operation: FileWatcherOperation::Remove,
                    path: from.as_path().to_path_buf(),
                    error: format!("{error:#}"),
                }));
            }
            worktree.root = new_root.clone();
            worktree.snapshot.set_root(new_root.clone());
            cx.emit(ProjectEvent::RootChanged(new_root.as_path().to_path_buf()));
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
        let path = normalize_for_comparison(path)?;
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
        let path = normalize_for_comparison(path)?;
        let root = self
            .root()
            .ok_or_else(|| anyhow::anyhow!("当前项目没有 worktree"))?;
        anyhow::ensure!(path.as_path() != root, "不能删除项目根目录");
        anyhow::ensure!(path.starts_with(root), "条目不在当前项目中");
        trash::delete(&path)?;
        self.buffer_store.remove_path(&path);
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
        let from = normalize_for_comparison(from)?;
        let to = normalize_for_comparison(to)?;
        anyhow::ensure!(from != to, "新旧路径不能相同");
        let root = self
            .root()
            .ok_or_else(|| anyhow::anyhow!("当前项目没有 worktree"))?
            .to_path_buf();
        anyhow::ensure!(from.as_path() != root, "不能移动项目根目录");
        anyhow::ensure!(from.starts_with(&root), "条目不在当前项目中");
        anyhow::ensure!(to.starts_with(&root), "目标不在当前项目中");
        anyhow::ensure!(!to.starts_with(&from), "不能把条目移动到自身内部");
        // 对称守卫：目标是源的祖先目录时，覆盖路径的「先删目标」会把源一起递归删掉。
        anyhow::ensure!(
            !from.starts_with(&to),
            "不能把条目移动到自身的祖先目录：{}",
            to.display()
        );
        if to.exists() {
            anyhow::ensure!(overwrite, "目标已存在：{}", to.display());
        }
        let indexed_from = AbsolutePathBuf::canonicalize(&from)?.into_path_buf();
        let parent = to
            .parent()
            .ok_or_else(|| anyhow::anyhow!("无法确定移动目标路径"))?;
        let name = to
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("无法确定移动目标路径"))?;
        // 目标父目录必须已存在：目录移动场景下由调用方保证，缺失时在 canonicalize 处报错。
        let indexed_to = AbsolutePathBuf::canonicalize(parent)?.as_path().join(name);
        // 优先直接 rename：同文件系统上 POSIX rename 原子替换文件/空目录目标，没有「先删后写」的危险中间态；
        // 仅当失败且目标是非空目录（rename 无法原地替换的唯一情形）才退化为「删目标再 rename」，最终失败经 Result 向上传播。
        if let Err(error) = std::fs::rename(&from, &to) {
            let is_nonempty_dir = to.is_dir()
                && std::fs::read_dir(&to).is_ok_and(|mut entries| entries.next().is_some());
            if !(overwrite && is_nonempty_dir) {
                return Err(error)
                    .with_context(|| format!("移动失败：{} → {}", from.display(), to.display()));
            }
            remove_entry(&to)?;
            std::fs::rename(&from, &to)
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
        let source = normalize_for_comparison(source)?;
        let destination = normalize_for_comparison(destination)?;
        let root = self
            .root()
            .ok_or_else(|| anyhow::anyhow!("当前项目没有 worktree"))?
            .to_path_buf();
        anyhow::ensure!(source.as_path() != root, "不能复制项目根目录");
        anyhow::ensure!(source.starts_with(&root), "条目不在当前项目中");
        anyhow::ensure!(destination.starts_with(&root), "目标不在当前项目中");
        anyhow::ensure!(
            !destination.starts_with(&source),
            "不能把条目复制到自身内部"
        );
        // 对称守卫：目标是源的祖先目录时，覆盖路径的「先删目标」会把源一起递归删掉。
        anyhow::ensure!(
            !source.starts_with(&destination),
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
        let source = source.as_path().to_path_buf();
        let destination = destination.as_path().to_path_buf();
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
                        Err(error) => Err(error),
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
            .map(|mut event| {
                let Ok(path) = normalize_for_comparison(&event.path) else {
                    return event;
                };
                event.path = path;
                event
            })
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
            .map(|event| event.path.clone().into_path_buf())
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

fn write_buffer_to_path(snapshot: &Snapshot, path: &Path) -> Result<(), BufferSaveError> {
    let version = snapshot.version();
    let mut file = File::create(path)?;
    write_buffer_to(snapshot, version, &mut file, LineEndingConfig::Preserve)?;
    file.sync_all()?;
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
        platform::create_symlink(source, &target, destination, file_type)?;
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
#[path = "test/project_store_tests.rs"]
mod tests;
