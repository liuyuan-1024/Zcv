//! Buffer 生命周期门面：构造 Buffer、暴露身份、只读状态、保存点和 dirty 判断。
//!
//! 本文件只管理 Buffer 作为文档对象的外部可见状态，不执行具体编辑、坐标转换或历史回放。

use super::{Buffer, history};
use crate::{
    BufferConfig, BufferId, BufferVersion, TextResult, TransactionId,
    storage::{RopeyStorage, TextRead},
    tracking::CoordinateIndex,
};

impl Buffer {
    /// 从已有文本创建匿名来源 Buffer。
    pub fn from_text(text: String, config: BufferConfig) -> TextResult<Self> {
        let storage = RopeyStorage::new(text);
        Ok(Self::from_parts(storage, config))
    }

    /// Buffer 构造的共享路径：绑定存储与默认状态。
    ///
    /// Buffer 字段默认值变更只需改这一处。
    fn from_parts(storage: RopeyStorage, config: BufferConfig) -> Self {
        let mut buffer = Self {
            buffer_id: BufferId::next_local(),
            read_only: false,
            config,
            storage,
            version: BufferVersion::INITIAL,
            saved_version: BufferVersion::INITIAL,
            next_transaction_id: TransactionId::INITIAL,
            text_changes: Default::default(),
            edit_log: Default::default(),
            coordinate_index: CoordinateIndex::default(),
            history: history::HistoryState::new(),
            session: None,
        };
        buffer.apply_large_file_auto_read_only();
        buffer
    }

    /// 加载 / 外部重置后按 `LargeFilePolicy::auto_read_only_on_large_file`
    /// 决定是否切到只读；只在大文件触发时把 `read_only` 置为 `true`，
    /// 不会主动取消既有的只读状态。
    pub(in crate::buffer) fn apply_large_file_auto_read_only(&mut self) {
        if self.config.large_file.auto_read_only_on_large_file && self.is_large_file() {
            self.read_only = true;
        }
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    #[cfg(test)]
    pub(crate) fn set_config(&mut self, config: BufferConfig) {
        self.config = config;
        self.truncate_edit_history_to_budget();
    }

    /// 当前 Buffer 文本字节数是否被 `LargeFilePolicy::large_file_threshold_bytes`
    /// 视为大文件。
    ///
    /// `large_file_threshold_bytes == 0` 时永远返回 `false`。
    pub fn is_large_file(&self) -> bool {
        self.config
            .large_file
            .is_large_byte_size(self.storage.len_bytes().get())
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    /// 与 Zed `Buffer::remote_id` 对齐的稳定 Buffer 身份。
    ///
    /// Zcv 当前不引入协作或远程同步；本地创建时分配进程内唯一 ID，重载文本不会改变它。
    pub fn buffer_id(&self) -> BufferId {
        self.buffer_id
    }

    pub fn saved_version(&self) -> BufferVersion {
        self.saved_version
    }

    /// 自保存点以来是否存在结构性文本编辑。
    ///
    /// 直接由 `edits_since(saved_version)` 派生，不再长期保留保存点全文快照或指纹。
    /// 与 Zed 的 `has_edits_since` 同语义：编辑互相抵消（如插入后撤销）为 clean，
    /// 替换后又替换回原文仍计为 dirty；保存点已退出编辑日志时保守判 dirty。
    pub fn is_dirty(&self) -> bool {
        if self.version == self.saved_version {
            return false;
        }

        // 保存点版本不可得时无法比较净值，按 dirty 处理，避免误报 clean。
        self.snapshot()
            .has_edits_since(self.saved_version)
            .unwrap_or(true)
    }

    pub fn mark_saved(&mut self) {
        self.saved_version = self.version;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ByteOffset, Edit, TransactionMetadata, TransactionSource};

    #[test]
    fn set_config_applies_the_new_history_budget_immediately() {
        let mut buffer =
            Buffer::from_text(String::new(), BufferConfig::default()).expect("空 Buffer 应能创建");
        buffer
            .edit(
                [Edit::insert(ByteOffset::ZERO, "a").expect("插入编辑必须合法")],
                TransactionMetadata::new(TransactionSource::Programmatic),
            )
            .expect("编辑应成功");
        assert!(buffer.can_undo());

        let mut config = buffer.config().clone();
        config.large_file.max_undo_history = 0;
        buffer.set_config(config);

        assert!(!buffer.can_undo());
    }
}
