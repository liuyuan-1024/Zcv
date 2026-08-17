//! 项目级状态与服务协调。
//!
//! `Project` 管理项目根、目录快照（Worktree）、文件 Buffer 生命周期和文件系统监听。
//! 窗口布局、Pane、Dock 与其他界面状态仍由 `Workspace` 管理。

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui::{App, AppContext, AsyncApp, Context, Entity, EventEmitter, Task, WeakEntity};
use zcv_engine::{Buffer, BufferLoadError, BufferSaveError};
use zcv_fs_watch::{FsWatcher, PathEvent, PathEventKind, Watcher};
use zcv_git::FileStatus;
use zcv_language::LanguageBuffer;

use super::buffer_store::BufferStore;
use super::git_store::{GitStore, StatusEntry};
use super::worktree::{Worktree, WorktreeEntry};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectEvent {
    RootChanged(PathBuf),
    EntriesChanged,
}

pub struct Project {
    root: PathBuf,
    /// 项目目录快照层（遍历/排除规则/路径语义），供项目树消费。
    worktree: Worktree,
    buffer_store: BufferStore,
    git_store: Entity<GitStore>,
    fs_watcher: Arc<dyn Watcher>,
    _fs_task: Task<()>,
}

impl Project {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let (signal_tx, signal_rx) = async_channel::unbounded::<()>();
        let pending_events = Arc::new(Mutex::new(Vec::new()));
        let fs_watcher: Arc<dyn Watcher> =
            Arc::new(FsWatcher::new(signal_tx, pending_events.clone()));

        if let Err(error) = fs_watcher.add(&root) {
            log::warn!("无法监听项目目录 {:?}：{error}", root);
        }

        let fs_task = cx.spawn(|project: WeakEntity<Project>, asynccx: &mut AsyncApp| {
            let mut cx = asynccx.clone();
            async move {
                while signal_rx.recv().await.is_ok() {
                    let events = std::mem::take(&mut *pending_events.lock().unwrap());
                    let _ = project.update(&mut cx, |project, cx| {
                        project.process_fs_events(events, cx);
                    });
                }
            }
        });

        let git_store = cx.new(|cx| GitStore::new(root.clone(), cx));
        git_store.update(cx, |store, cx| store.schedule_scan(cx));

        Self {
            root: root.clone(),
            worktree: Worktree::new(root),
            buffer_store: BufferStore::new(),
            git_store,
            fs_watcher,
            _fs_task: fs_task,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 更新项目树的扫描排除规则（设置变化时由项目树调用）。
    pub fn set_exclusions(&mut self, exclusions: &[String]) {
        self.worktree.set_exclusions(exclusions);
    }

    /// 查询目录的直接子项：worktree 读取 + git 状态合并。
    ///
    /// git 状态现查 `GitStore`（目录行聚合、文件行精确），展开产生的新行因此立即携带状态，无需二次补齐。
    /// 展开、深度与可见行是视图状态，由项目树 UI 层自行构建。
    pub fn children(&self, path: &Path, cx: &App) -> Vec<WorktreeEntry> {
        self.worktree
            .children(path)
            .into_iter()
            .map(|mut entry| {
                entry.git_status = if entry.is_dir {
                    self.git_status_for_directory(&entry.path, cx)
                } else {
                    self.git_status_for_path(&entry.path, cx).map(|e| e.status)
                };
                entry
            })
            .collect()
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

    pub fn open_buffer(
        &mut self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Result<Entity<LanguageBuffer>, BufferLoadError> {
        self.buffer_store.open_buffer(path, cx)
    }

    pub fn save_buffer(
        &mut self,
        buffer: &Entity<Buffer>,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Result<(), BufferSaveError> {
        let result = buffer.update(cx, |buffer, cx| {
            let result = write_buffer_to_path(buffer, path);
            if result.is_ok() {
                cx.notify();
            }
            result
        });
        // 保存成功后立即刷新 git 状态（快路径，不等 fs 事件；
        // fs 事件晚到会被 job 去重吸收）。
        if result.is_ok() {
            self.git_store.update(cx, |store, cx| {
                store.refresh_statuses_for_paths(std::slice::from_ref(&path.to_path_buf()), cx);
            });
        }
        result
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
        anyhow::ensure!(
            from == self.root || from.starts_with(&self.root),
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

        if from == self.root {
            if let Err(error) = self.fs_watcher.add(to) {
                log::warn!("无法监听重命名后的项目目录 {:?}：{error}", to);
            }
            if let Err(error) = self.fs_watcher.remove(from) {
                log::warn!("无法停止监听旧项目目录 {:?}：{error}", from);
            }
            self.root = to.to_path_buf();
            self.worktree.set_root(to.to_path_buf());
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
        let relative = path
            .strip_prefix(&self.root)
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
        anyhow::ensure!(path != self.root, "不能删除项目根目录");
        anyhow::ensure!(path.starts_with(&self.root), "条目不在当前项目中");
        trash::delete(path)?;
        self.buffer_store.remove_path(path);
        cx.emit(ProjectEvent::EntriesChanged);
        Ok(())
    }

    fn process_fs_events(&mut self, events: Vec<PathEvent>, cx: &mut Context<Self>) {
        let events: Vec<_> = events
            .into_iter()
            .filter(|event| event.path.starts_with(&self.root))
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use gpui::AppContext;
    use zcv_engine::{BufferConfig, ByteOffset, Edit, TransactionMetadata};

    use super::*;
    use crate::test_support::test_git_repo;

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
        let engine_buffer = cx.read_entity(&buffer, |language_buffer, _| language_buffer.buffer());
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
            .update(cx, |project, cx| {
                project.save_buffer(&engine_buffer, &file, cx)
            })
            .expect("保存应成功");
        cx.run_until_parked();
        let entry = project
            .update(cx, |project, cx| project.git_status_for_path(&file, cx))
            .expect("保存后应有 git 状态");
        assert!(entry.status.is_modified());
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
