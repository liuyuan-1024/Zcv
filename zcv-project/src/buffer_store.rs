//! 文件路径到共享 MultiBuffer 文档实体的索引。
//!
//! Store 只保留弱引用；只要还有 Editor 或 View 持有文档，它就能按路径复用，最后一个使用者释放后，整条文档实体链也随之结束。

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::translate_path;
use gpui::{App, AppContext, Entity, WeakEntity};
use zcv_language::LanguageBuffer;
use zcv_multi_buffer::MultiBuffer;
use zcv_text::Snapshot;
use zcv_text::{Buffer, BufferConfig, BufferLoadError};

pub(crate) struct BufferStore {
    opened_buffers: HashMap<PathBuf, WeakEntity<MultiBuffer>>,
}

impl BufferStore {
    pub(crate) fn new() -> Self {
        Self {
            opened_buffers: HashMap::new(),
        }
    }

    /// 打开文件；同一个规范化路径始终复用仍然存活的 MultiBuffer。
    pub(crate) fn open_buffer(
        &mut self,
        path: &Path,
        cx: &mut App,
    ) -> Result<Entity<MultiBuffer>, BufferLoadError> {
        let path = path.canonicalize().map_err(BufferLoadError::Io)?;
        if let Some(buffer) = self.opened_buffers.get(&path).and_then(WeakEntity::upgrade) {
            return Ok(buffer);
        }

        let file = File::open(&path).map_err(BufferLoadError::Io)?;
        let buffer = Buffer::from_reader(file, BufferConfig::default())?;
        let buffer = cx.new(|_| buffer);
        let language_buffer = cx.new(|cx| LanguageBuffer::new(buffer, Some(path.clone()), cx));
        let multi_buffer = cx.new(|cx| MultiBuffer::singleton(language_buffer, cx));
        self.opened_buffers.insert(path, multi_buffer.downgrade());
        Ok(multi_buffer)
    }

    pub(crate) fn opened_snapshots(&self, cx: &App) -> HashMap<PathBuf, Snapshot> {
        self.opened_buffers
            .iter()
            .filter_map(|(path, buffer)| {
                let buffer = buffer.upgrade()?;
                Some((path.clone(), buffer.read(cx).snapshot(cx).text().clone()))
            })
            .collect()
    }

    /// 如果路径对应某个已打开的 Buffer，从磁盘重新加载其内容。
    pub(crate) fn reload_buffer_for_path(&mut self, path: &Path, cx: &mut App) {
        let Ok(canonical) = path.canonicalize() else {
            return;
        };
        let Some(multi_buffer) = self
            .opened_buffers
            .get(&canonical)
            .and_then(WeakEntity::upgrade)
        else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(&canonical) else {
            return;
        };
        let buffer = multi_buffer
            .read(cx)
            .as_singleton(cx)
            .expect("当前 BufferStore 只创建 singleton MultiBuffer");
        buffer.update(cx, |buffer, cx| {
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use gpui::{AppContext, TestAppContext};

    use super::*;

    static NEXT_TEST_FILE_ID: AtomicU64 = AtomicU64::new(1);

    #[gpui::test]
    fn opening_the_same_file_reuses_its_buffer(cx: &mut TestAppContext) {
        let path = test_file_path();
        fs::write(&path, "共享内容").expect("测试文件应可写入");

        let (first, second) = cx.update(|cx| {
            let mut store = BufferStore::new();
            let first = store.open_buffer(&path, cx).expect("首次打开应成功");
            let second = store.open_buffer(&path, cx).expect("再次打开应成功");
            (first, second)
        });

        assert_eq!(first, second);
        fs::remove_file(path).expect("测试文件应可删除");
    }

    #[gpui::test]
    fn separate_project_buffer_stores_do_not_share_buffers(cx: &mut TestAppContext) {
        let path = test_file_path();
        fs::write(&path, "项目隔离").expect("测试文件应可写入");

        let (first, second) = cx.update(|cx| {
            let mut first_store = BufferStore::new();
            let mut second_store = BufferStore::new();
            let first = first_store
                .open_buffer(&path, cx)
                .expect("第一项目应打开文件");
            let second = second_store
                .open_buffer(&path, cx)
                .expect("第二项目应打开文件");
            (first, second)
        });

        assert_ne!(first, second);
        fs::remove_file(path).expect("测试文件应可删除");
    }

    #[gpui::test]
    fn remove_path_drops_matching_indexes_and_keeps_others(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let directory = directory.path().canonicalize().expect("临时目录应可规范化");
        let file = directory.join("file.txt");
        let nested = directory.join("sub").join("nested.txt");
        let sibling = directory.join("sibling.txt");
        fs::create_dir_all(directory.join("sub")).expect("应创建子目录");
        fs::write(&file, "文件").expect("应创建测试文件");
        fs::write(&nested, "嵌套").expect("应创建测试文件");
        fs::write(&sibling, "同级").expect("应创建测试文件");

        let mut store = BufferStore::new();
        let (file_buffer, nested_buffer, sibling_buffer) = cx.update(|cx| {
            (
                store.open_buffer(&file, cx).expect("应打开测试文件"),
                store.open_buffer(&nested, cx).expect("应打开测试文件"),
                store.open_buffer(&sibling, cx).expect("应打开测试文件"),
            )
        });

        // 精确文件删除只移除该文件的索引；目录删除连同其中已打开的 Buffer 一起移除。
        store.remove_path(&file);
        store.remove_path(&directory.join("sub"));

        let (reloaded_file, reloaded_nested, kept_sibling) = cx.update(|cx| {
            (
                store.open_buffer(&file, cx).expect("应重新加载测试文件"),
                store.open_buffer(&nested, cx).expect("应重新加载测试文件"),
                store.open_buffer(&sibling, cx).expect("应重新打开测试文件"),
            )
        });
        assert_ne!(file_buffer, reloaded_file, "被删文件的索引应被移除");
        assert_ne!(
            nested_buffer, reloaded_nested,
            "被删目录内 Buffer 的索引应被移除"
        );
        assert_eq!(sibling_buffer, kept_sibling, "未匹配的索引应保留并复用");
        fs::remove_file(file).expect("测试文件应可删除");
        fs::remove_file(nested).expect("测试文件应可删除");
        fs::remove_file(sibling).expect("测试文件应可删除");
    }

    #[gpui::test]
    fn released_buffer_is_loaded_again(cx: &mut TestAppContext) {
        let path = test_file_path();
        fs::write(&path, "第一次").expect("测试文件应可写入");

        let mut store = BufferStore::new();
        let first_id = cx.update(|cx| {
            let buffer = store.open_buffer(&path, cx).expect("首次打开应成功");
            buffer.entity_id()
        });
        cx.run_until_parked();

        fs::write(&path, "第二次").expect("测试文件应可更新");
        let second = cx.update(|cx| store.open_buffer(&path, cx).expect("重新打开应成功"));

        assert_ne!(first_id, second.entity_id());
        let buffer = cx.read_entity(&second, |multi_buffer, cx| {
            multi_buffer
                .as_singleton(cx)
                .expect("测试文档应是 singleton")
        });
        cx.read_entity(&buffer, |buffer, _| {
            assert_eq!(
                buffer
                    .slice_byte_range(zcv_text::ByteOffset::ZERO, buffer.len_bytes())
                    .expect("完整 Buffer 应可读取")
                    .as_str(),
                "第二次"
            );
        });
        fs::remove_file(path).expect("测试文件应可删除");
    }

    fn test_file_path() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应晚于 Unix Epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "buffer-store-test-{}-{nonce}-{}.txt",
            std::process::id(),
            NEXT_TEST_FILE_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
