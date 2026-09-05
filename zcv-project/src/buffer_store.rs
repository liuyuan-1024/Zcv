//! 文件路径到共享 LanguageBuffer 文档实体的索引。
//!
//! Store 只保留弱引用；只要还有 Editor 或 View 持有文档，它就能按路径复用，最后一个使用者释放后，整条文档实体链也随之结束。

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::translate_path;
use gpui::{App, AppContext, Entity, WeakEntity};
use zcv_language::LanguageBuffer;
use zcv_text::Snapshot;
use zcv_text::{Buffer, BufferConfig, BufferLoadError};

pub(crate) struct BufferStore {
    opened_buffers: HashMap<PathBuf, WeakEntity<LanguageBuffer>>,
}

impl BufferStore {
    pub(crate) fn new() -> Self {
        Self {
            opened_buffers: HashMap::new(),
        }
    }

    /// 打开文件；同一个规范化路径始终复用仍然存活的 LanguageBuffer。
    pub(crate) fn open_buffer(
        &mut self,
        path: &Path,
        cx: &mut App,
    ) -> Result<Entity<LanguageBuffer>, BufferLoadError> {
        self.get_or_load_buffer(
            path,
            || {
                let file = File::open(path).map_err(BufferLoadError::Io)?;
                Buffer::from_reader(file, BufferConfig::default())
            },
            cx,
        )
    }

    /// 打开工作区侧已经不存在的文件。
    ///
    /// 删除状态的 Git 变更仍需要一个空的工作区 Buffer 作为可编辑侧；
    /// HEAD 内容由差异模型单独提供。
    /// 若用户在该位置输入并保存，文件会按正常保存路径重新创建。
    pub(crate) fn open_deleted_buffer(
        &mut self,
        path: &Path,
        cx: &mut App,
    ) -> Result<Entity<LanguageBuffer>, BufferLoadError> {
        self.get_or_load_buffer(
            path,
            || {
                Ok(Buffer::scratch(String::new(), BufferConfig::default())
                    .expect("空的删除文件 Buffer 应能创建"))
            },
            cx,
        )
    }

    /// 注册搜索任务在后台加载完成的 Buffer，与 `open_buffer` 共享同一缓存。
    pub(crate) fn register_loaded_buffer(
        &mut self,
        path: PathBuf,
        buffer: Buffer,
        cx: &mut App,
    ) -> Result<Entity<LanguageBuffer>, BufferLoadError> {
        self.get_or_load_buffer(&path, || Ok(buffer), cx)
    }

    /// 索引命中时不加载内容；磁盘加载与后台结果注册共用同一文档身份入口。
    fn get_or_load_buffer(
        &mut self,
        path: &Path,
        load: impl FnOnce() -> Result<Buffer, BufferLoadError>,
        cx: &mut App,
    ) -> Result<Entity<LanguageBuffer>, BufferLoadError> {
        let path = index_path(path).map_err(BufferLoadError::Io)?;
        if let Some(buffer) = self.opened_buffers.get(&path).and_then(WeakEntity::upgrade) {
            return Ok(buffer);
        }
        let buffer = load()?;
        let buffer = cx.new(|_| buffer);
        let language_buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(path.clone()), cx));
        self.opened_buffers
            .insert(path, language_buffer.downgrade());
        Ok(language_buffer)
    }

    pub(crate) fn opened_snapshots(&self, cx: &App) -> HashMap<PathBuf, Snapshot> {
        self.opened_buffers
            .iter()
            .filter_map(|(path, buffer)| {
                let buffer = buffer.upgrade()?;
                Some((path.clone(), buffer.read(cx).text_snapshot(cx)))
            })
            .collect()
    }

    /// 如果路径对应某个干净的已打开 Buffer，从磁盘重新加载其内容。
    /// 脏 Buffer 由用户编辑拥有，文件事件不能覆盖它。
    pub(crate) fn reload_buffer_for_path(&mut self, path: &Path, cx: &mut App) {
        let Ok(canonical) = path.canonicalize() else {
            return;
        };
        let Some(language_buffer) = self
            .opened_buffers
            .get(&canonical)
            .and_then(WeakEntity::upgrade)
        else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(&canonical) else {
            return;
        };
        let buffer = language_buffer.read(cx).buffer();
        buffer.update(cx, |buffer, cx| {
            // 脏 Buffer 的文本由用户编辑拥有；文件事件不能用磁盘内容覆盖它。
            // 保存产生的延迟事件也可能在用户已经继续编辑或撤销后到达。
            if buffer.is_dirty() {
                return;
            }
            if buffer.reload_from_text(text).is_ok() {
                cx.notify();
            }
        });
    }

    /// 将已打开 Buffer 的路径索引随文件或目录重命名一起迁移。
    pub(crate) fn rename_path(&mut self, from: &Path, to: &Path) {
        self.opened_buffers = self
            .opened_buffers
            .drain()
            .map(|(path, buffer)| (translate_path(&path, from, to), buffer))
            .collect();
    }

    /// 移除被删除文件或目录对应的路径索引；目录删除时连同其中已打开的 Buffer 一起移除。
    pub(crate) fn remove_path(&mut self, path: &Path) {
        self.opened_buffers
            .retain(|indexed, _| indexed.strip_prefix(path).is_err());
    }
}

/// 为已存在与刚删除的文件生成同一种规范化索引路径。
fn index_path(path: &Path) -> std::io::Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) => {
            let Some(parent) = path.parent() else {
                return Err(error);
            };
            let Some(file_name) = path.file_name() else {
                return Err(error);
            };
            parent.canonicalize().map(|parent| parent.join(file_name))
        }
    }
}

#[cfg(test)]
#[path = "test/buffer_store_tests.rs"]
mod tests;
