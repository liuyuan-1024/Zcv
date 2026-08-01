//! 文件路径到共享 Buffer Entity 的索引。
//!
//! Store 只保留弱引用；只要还有 Editor 或 View 持有 Buffer，它就能按路径复用，
//! 最后一个使用者释放后，Buffer 的生命周期也随之结束。

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use gpui::{App, AppContext, Entity, WeakEntity};
use zcv_engine::{Buffer, BufferConfig, BufferLoadError, BufferOrigin};

pub(crate) struct BufferStore {
    opened_buffers: HashMap<PathBuf, WeakEntity<Buffer>>,
}

impl BufferStore {
    pub(crate) fn new() -> Self {
        Self {
            opened_buffers: HashMap::new(),
        }
    }

    /// 打开文件；同一个规范化路径始终复用仍然存活的 Buffer。
    pub(crate) fn open_buffer(
        &mut self,
        path: &Path,
        cx: &mut App,
    ) -> Result<Entity<Buffer>, BufferLoadError> {
        let path = path.canonicalize().map_err(BufferLoadError::Io)?;
        if let Some(buffer) = self.opened_buffers.get(&path).and_then(WeakEntity::upgrade) {
            return Ok(buffer);
        }

        let file = File::open(&path).map_err(BufferLoadError::Io)?;
        let buffer = Buffer::from_reader(
            BufferOrigin::external(path.to_string_lossy()),
            file,
            BufferConfig::default(),
        )?;
        let buffer = cx.new(|_| buffer);
        self.opened_buffers.insert(path, buffer.downgrade());
        Ok(buffer)
    }

    /// 如果路径对应某个已打开的 Buffer，从磁盘重新加载其内容。
    pub(crate) fn reload_buffer_for_path(&mut self, path: &Path, cx: &mut App) {
        let Ok(canonical) = path.canonicalize() else {
            return;
        };
        let Some(buffer) = self
            .opened_buffers
            .get(&canonical)
            .and_then(WeakEntity::upgrade)
        else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(&canonical) else {
            return;
        };
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
            .map(|(path, buffer)| {
                let path = path
                    .strip_prefix(from)
                    .map_or(path.clone(), |suffix| to.join(suffix));
                (path, buffer)
            })
            .collect();
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
        cx.read_entity(&second, |buffer, _| {
            assert_eq!(
                buffer
                    .slice_byte_range(zcv_engine::ByteOffset::ZERO, buffer.len_bytes())
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
            "zcv-buffer-store-{}-{nonce}-{}.txt",
            std::process::id(),
            NEXT_TEST_FILE_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
