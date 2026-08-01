//! 项目级状态与服务协调。
//!
//! `Project` 管理项目根、文件 Buffer 生命周期和文件系统监听。窗口布局、Pane、
//! Dock 与其他界面状态仍由 `Workspace` 管理。

mod buffer_store;

use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui::{AsyncApp, Context, Entity, EventEmitter, Task, WeakEntity};
use zcv_engine::{Buffer, BufferLoadError, BufferSaveError};
use zcv_language::LanguageBuffer;

use self::buffer_store::BufferStore;
use crate::fs_watcher::{FsWatcher, PathEvent, PathEventKind, Watcher};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectEvent {
    RootChanged(PathBuf),
    EntriesChanged,
}

pub(crate) struct Project {
    root: PathBuf,
    buffer_store: BufferStore,
    fs_watcher: Arc<dyn Watcher>,
    _fs_task: Task<()>,
}

impl Project {
    pub(crate) fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let (signal_tx, signal_rx) = async_channel::unbounded::<()>();
        let pending_events = Arc::new(Mutex::new(Vec::new()));
        let fs_watcher: Arc<dyn Watcher> =
            Arc::new(FsWatcher::new(signal_tx, pending_events.clone()));

        if let Err(error) = fs_watcher.add(&root) {
            log::warn!("无法监听项目目录 {:?}：{error}", root);
        }

        let fs_task = cx.spawn(|project: WeakEntity<Project>, async_cx: &mut AsyncApp| {
            let mut cx = async_cx.clone();
            async move {
                while signal_rx.recv().await.is_ok() {
                    let events = std::mem::take(&mut *pending_events.lock().unwrap());
                    let _ = project.update(&mut cx, |project, cx| {
                        project.process_fs_events(events, cx);
                    });
                }
            }
        });

        Self {
            root,
            buffer_store: BufferStore::new(),
            fs_watcher,
            _fs_task: fs_task,
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let root = root.canonicalize()?;
        anyhow::ensure!(root.is_dir(), "项目根必须是目录：{}", root.display());
        if root == self.root {
            return Ok(());
        }

        self.fs_watcher.add(&root)?;
        if let Err(error) = self.fs_watcher.remove(&self.root) {
            log::warn!("无法停止监听旧项目目录 {:?}：{error}", self.root);
        }
        self.root = root.clone();
        self.buffer_store = BufferStore::new();
        cx.emit(ProjectEvent::RootChanged(root));
        Ok(())
    }

    pub(crate) fn open_buffer(
        &mut self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Result<Entity<LanguageBuffer>, BufferLoadError> {
        self.buffer_store.open_buffer(path, cx)
    }

    pub(crate) fn save_buffer(
        &mut self,
        buffer: &Entity<Buffer>,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Result<(), BufferSaveError> {
        buffer.update(cx, |buffer, cx| {
            let result = write_buffer_to_path(buffer, path);
            if result.is_ok() {
                cx.notify();
            }
            result
        })
    }

    /// 在同一父目录内重命名文件或目录，并迁移项目持有的路径状态。
    pub(crate) fn rename_path(
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
            cx.emit(ProjectEvent::RootChanged(to.to_path_buf()));
        } else {
            cx.emit(ProjectEvent::EntriesChanged);
        }
        Ok(())
    }

    /// 在项目内新建一个空文件或目录，并补齐缺失的父目录。
    pub(crate) fn create_path(
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
        cx.emit(ProjectEvent::EntriesChanged);
    }
}

impl EventEmitter<ProjectEvent> for Project {}

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
    use zcv_engine::{BufferConfig, ByteOffset};

    use super::*;

    #[test]
    fn saving_buffer_writes_current_version_and_marks_it_clean() {
        let path = test_file_path();
        let mut buffer =
            Buffer::scratch("旧内容".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
        buffer
            .insert(buffer.len_bytes(), " + 新内容")
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
            .insert(ByteOffset::ZERO, "未保存")
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

    fn test_file_path() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应晚于 Unix Epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "zcv-project-save-{}-{nonce}.txt",
            std::process::id()
        ))
    }
}
