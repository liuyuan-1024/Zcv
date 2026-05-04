use crate::{BufferVersion, Snapshot, storage::TextStorage};

use super::Buffer;

impl Buffer {
    /// 创建绑定当前版本的不可变快照。
    ///
    /// M4 后，底层直接通过 `RopeyStorage::snapshot()` 获取基于 `ropey::Rope::clone()`
    /// 的低成本快照；这里仅负责把快照与 BufferVersion / BufferConfig 绑定成 public Snapshot。
    pub fn snapshot(&self) -> Snapshot {
        Snapshot::new(self.storage.snapshot(), self.version, self.config.clone())
    }

    /// 判断给定版本是否已经相对当前 Buffer 过期。
    pub fn is_version_stale(&self, version: BufferVersion) -> bool {
        version != self.version
    }

    pub fn is_snapshot_stale(&self, snapshot: &Snapshot) -> bool {
        snapshot.version() != self.version
    }
}
