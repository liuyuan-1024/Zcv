//! BufferStore —— 文件路径到共享 Buffer Entity 的索引。
//!
//! Store 只保留弱引用；
//! 只要还有 Editor 或 View 持有 Buffer，它就能按路径复用，最后一个使用者释放后，Buffer 的生命周期也随之结束。

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use gpui::{App, AppContext, Entity, Global, WeakEntity};
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
}

impl Global for BufferStore {}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use gpui::{AppContext, TestAppContext};

    use super::*;

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
            "zcv-buffer-store-{}-{nonce}.txt",
            std::process::id()
        ))
    }
}
