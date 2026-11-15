//! 按源文档调度自动保存，落盘复用 Project 的文件保存流程。

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Seek},
    path::{Path, PathBuf},
    time::Duration,
};

use gpui::{Context, Entity, EntityId};
use sha2::{Digest, Sha256};
use zcv_language::LanguageBuffer;
use zcv_settings::{AutosaveSetting, SettingsStore};
use zcv_text::{ByteOffset, Snapshot, TextRange, TextRead};

use super::{BufferLoadError, BufferSaveError, Project, ProjectEvent, SaveMode, decode_to_string};

impl Project {
    pub(super) fn remember_disk_text(&mut self, id: EntityId, snapshot: &Snapshot) {
        let range =
            TextRange::new(ByteOffset::ZERO, snapshot.len_bytes()).expect("文件快照范围必须有序");
        let mut hash = Sha256::new();
        for chunk in snapshot.chunks(range).expect("文件快照必须可读取") {
            hash.update(chunk.as_bytes());
        }
        self.disk_text_hashes.insert(id, hash.finalize().into());
    }

    pub(super) fn update_autosave_setting(&mut self, cx: &mut Context<Self>) {
        self.autosave_tasks.clear();
        if let AutosaveSetting::AfterDelay { .. } = SettingsStore::autosave(cx) {
            for (buffer, _) in self.buffer_store.opened_file_buffers() {
                self.schedule_autosave(&buffer, cx);
            }
        }
    }

    pub(super) fn schedule_autosave(
        &mut self,
        buffer: &Entity<LanguageBuffer>,
        cx: &mut Context<Self>,
    ) {
        let id = buffer.entity_id();
        self.autosave_tasks.remove(&id);
        let AutosaveSetting::AfterDelay { milliseconds } = SettingsStore::autosave(cx) else {
            return;
        };
        let source = buffer.read(cx);
        if !source.is_dirty() || source.is_read_only() || !self.disk_text_hashes.contains_key(&id) {
            return;
        }
        let buffer = buffer.downgrade();
        let timer = cx
            .background_executor()
            .timer(Duration::from_millis(milliseconds.get()));
        let task = cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |project, cx| {
                if let Some(task) = project.autosave_tasks.remove(&id) {
                    task.detach();
                }
                if SettingsStore::autosave(cx) != (AutosaveSetting::AfterDelay { milliseconds }) {
                    return;
                }
                if let Some(buffer) = buffer.upgrade()
                    && let Some(path) = project.buffer_store.path_for_buffer(&buffer)
                {
                    project.autosave_file_buffers(vec![(buffer, path.into_path_buf())], cx);
                }
            })
            .ok();
        });
        self.autosave_tasks.insert(id, task);
    }

    /// 保存当前项目中仍存活的可写文件文档。
    pub fn autosave_open_buffers(&mut self, cx: &mut Context<Self>) -> bool {
        let buffers = self.buffer_store.opened_file_buffers();
        self.autosave_file_buffers(buffers, cx)
    }

    /// 自动保存仅提交脏的可写源文档。冲突与错误逐个上报，其余源仍可保存。
    pub fn autosave_file_buffers(
        &mut self,
        buffers: Vec<(Entity<LanguageBuffer>, PathBuf)>,
        cx: &mut Context<Self>,
    ) -> bool {
        let setting = SettingsStore::autosave(cx);
        if setting == AutosaveSetting::Off {
            return true;
        }
        let format = !matches!(setting, AutosaveSetting::AfterDelay { .. });
        let mut saved = true;
        for (buffer, path) in buffers {
            let source = buffer.read(cx);
            if !source.is_dirty() || source.is_read_only() {
                continue;
            }
            let id = buffer.entity_id();
            if let Err(error) = self.save_file_buffers_with_mode(
                vec![(buffer, path.clone())],
                SaveMode::Automatic { format },
                cx,
            ) {
                saved = false;
                cx.emit(ProjectEvent::AutosaveFailed {
                    buffer_id: id,
                    path,
                    conflict: matches!(error, BufferSaveError::Conflict(_)),
                    error: error.to_string(),
                });
            }
        }
        saved
    }

    pub(super) fn disk_matches_baseline(&self, buffer: &Entity<LanguageBuffer>) -> bool {
        let Some(baseline) = self.disk_text_hashes.get(&buffer.entity_id()) else {
            return false;
        };
        let Some(path) = self.buffer_store.path_for_buffer(buffer) else {
            return false;
        };
        open_regular_file(path.as_path(), OpenOptions::new().read(true))
            .and_then(|file| match file {
                Some(mut file) => disk_text_hash(&mut file),
                None => Ok(None),
            })
            .is_ok_and(|hash| hash.as_ref() == Some(baseline))
    }

    pub(super) fn open_autosave_target(
        &self,
        buffer: &Entity<LanguageBuffer>,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Result<File, BufferSaveError> {
        if let Some(baseline) = self.disk_text_hashes.get(&buffer.entity_id())
            && let Some(mut file) =
                open_regular_file(path, OpenOptions::new().read(true).write(true))?
            && disk_text_hash(&mut file)?.as_ref() == Some(baseline)
        {
            file.rewind()?;
            return Ok(file);
        }
        buffer.update(cx, LanguageBuffer::file_changed_on_disk);
        Err(BufferSaveError::Conflict(path.to_path_buf()))
    }
}

fn open_regular_file(path: &Path, options: &OpenOptions) -> io::Result<Option<File>> {
    let result: io::Result<_> = (|| {
        if !fs::symlink_metadata(path)?.is_file() {
            return Ok(None);
        }
        let file = options.open(path)?;
        Ok(file.metadata()?.is_file().then_some(file))
    })();
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        result => result,
    }
}

fn disk_text_hash(file: &mut File) -> io::Result<Option<[u8; 32]>> {
    match decode_to_string(file) {
        Ok(text) => Ok(Some(Sha256::digest(text.as_bytes()).into())),
        Err(BufferLoadError::Io(error)) => Err(error),
        Err(_) => Ok(None),
    }
}
