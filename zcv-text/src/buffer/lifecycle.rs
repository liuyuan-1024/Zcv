//! Buffer 生命周期门面：构造 Buffer、暴露身份、只读状态、保存点和 dirty 判断。
//!
//! 本文件只管理 Buffer 作为文档对象的外部可见状态，不执行具体编辑、坐标转换或历史回放。

use super::{Buffer, history};
use crate::{
    BufferConfig, BufferGeneration, BufferVersion, TextResult, TransactionId,
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
            read_only: false,
            config,
            storage,
            version: BufferVersion::INITIAL,
            generation: BufferGeneration::INITIAL,
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

    pub fn set_config(&mut self, config: BufferConfig) {
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

        match self.edit_log.batch_since(self.saved_version, self.version) {
            Ok(batch) => !batch.patch().is_empty(),
            Err(_) => true,
        }
    }

    pub fn mark_saved(&mut self) {
        self.saved_version = self.version;
    }
}
