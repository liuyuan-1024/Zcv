//! 项目级状态与服务协调。
//!
//! `Project` 管理项目根、文件 Buffer 生命周期和文件系统监听。窗口布局、Pane、
//! Dock 与其他界面状态仍由 `Workspace` 管理。

mod buffer_store;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui::{AsyncApp, Context, Entity, EventEmitter, Task, WeakEntity};
use zcv_engine::{Buffer, BufferLoadError, BufferSaveError};

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
        if root == self.root {
            return Ok(());
        }

        self.fs_watcher.add(&root)?;
        if let Err(error) = self.fs_watcher.remove(&self.root) {
            log::warn!("无法停止监听旧项目目录 {:?}：{error}", self.root);
        }
        self.root = root.clone();
        cx.emit(ProjectEvent::RootChanged(root));
        Ok(())
    }

    pub(crate) fn open_buffer(
        &mut self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Result<Entity<Buffer>, BufferLoadError> {
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
