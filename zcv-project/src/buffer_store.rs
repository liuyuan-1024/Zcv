//! 文件路径到共享 LanguageBuffer 文档实体的索引。
//!
//! Store 只保留弱引用；只要还有 Editor 或 View 持有文档，它就能按路径复用，最后一个使用者释放后，整条文档实体链也随之结束。

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use crate::translate_path;
use gpui::{App, AppContext, Entity, EntityId, WeakEntity};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_path::{AbsolutePathBuf, normalize_for_comparison};
use zcv_text::Snapshot;
use zcv_text::{Buffer, BufferConfig};

use crate::text_file::{BufferLoadError, decode_to_string};

/// 从磁盘读取并解码文件，创建文本 `Buffer`。
///
/// 这是文件解码与文本 `Buffer` 创建的唯一入口：
/// `open_buffer` 与后台搜索都经这里，保证 BOM 剥离与非法 UTF-8 拒绝策略一致。
pub(crate) fn load_buffer(path: &Path) -> Result<Buffer, BufferLoadError> {
    let file = File::open(path)?;
    let text = decode_to_string(file)?;
    Buffer::from_text(text, BufferConfig::default()).map_err(BufferLoadError::Text)
}

pub(crate) struct BufferStore {
    opened_buffers: HashMap<AbsolutePathBuf, WeakEntity<LanguageBuffer>>,
    language_registry: Arc<LanguageRegistry>,
}

impl BufferStore {
    pub(crate) fn new(language_registry: Arc<LanguageRegistry>) -> Self {
        Self {
            opened_buffers: HashMap::new(),
            language_registry,
        }
    }

    /// 打开文件；同一个规范化路径始终复用仍然存活的 LanguageBuffer。
    pub(crate) fn open_buffer(
        &mut self,
        path: &Path,
        cx: &mut App,
    ) -> Result<Entity<LanguageBuffer>, BufferLoadError> {
        self.get_or_load_buffer(path, || load_buffer(path), cx)
    }

    pub(crate) fn install_loaded_buffer(
        &mut self,
        path: &Path,
        buffer: Buffer,
        cx: &mut App,
    ) -> Result<Entity<LanguageBuffer>, BufferLoadError> {
        self.get_or_load_buffer(path, || Ok(buffer), cx)
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
                Ok(Buffer::from_text(String::new(), BufferConfig::default())
                    .expect("空的删除文件 Buffer 应能创建"))
            },
            cx,
        )
    }

    /// 索引命中时不加载内容；磁盘加载只经 `load_buffer` 这一文件解码入口。
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
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                buffer,
                Some(path.as_path().to_path_buf()),
                Arc::clone(&self.language_registry),
                cx,
            )
        });
        self.opened_buffers
            .insert(path, language_buffer.downgrade());
        Ok(language_buffer)
    }

    pub(crate) fn opened_snapshots(&self, cx: &App) -> HashMap<AbsolutePathBuf, Snapshot> {
        self.opened_buffers
            .iter()
            .filter_map(|(path, buffer)| {
                let buffer = buffer.upgrade()?;
                Some((path.clone(), buffer.read(cx).text_snapshot()))
            })
            .collect()
    }

    /// 文件事件可能只报告目录或 Rescan 根；只返回受该路径覆盖的存活文档。
    pub(crate) fn buffers_under_path(&self, path: &Path) -> Vec<Entity<LanguageBuffer>> {
        self.opened_buffers
            .iter()
            .filter(|(opened_path, _)| opened_path.starts_with(path))
            .filter_map(|(_, buffer)| buffer.upgrade())
            .collect()
    }

    pub(crate) fn path_for_buffer(
        &self,
        buffer: &Entity<LanguageBuffer>,
    ) -> Option<AbsolutePathBuf> {
        self.opened_buffers.iter().find_map(|(path, opened)| {
            (opened.upgrade().as_ref() == Some(buffer)).then(|| path.clone())
        })
    }

    pub(crate) fn release_buffer(&mut self, id: EntityId) {
        self.opened_buffers.retain(|_, opened| {
            opened
                .upgrade()
                .is_some_and(|buffer| buffer.entity_id() != id)
        });
    }

    /// 将已打开 Buffer 的路径索引随文件或目录重命名一起迁移。
    pub(crate) fn rename_path(&mut self, from: &Path, to: &Path) {
        self.opened_buffers = self
            .opened_buffers
            .drain()
            .filter_map(|(path, buffer)| {
                let path = AbsolutePathBuf::new(translate_path(path.as_path(), from, to)).ok()?;
                Some((path, buffer))
            })
            .collect();
    }
}

/// 为已存在与刚删除的文件生成同一种规范化索引路径。
fn index_path(path: &Path) -> std::io::Result<AbsolutePathBuf> {
    normalize_for_comparison(path)
}

#[cfg(test)]
#[path = "test/buffer_store_tests.rs"]
mod tests;
