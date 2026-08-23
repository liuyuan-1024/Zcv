//! Buffer 生命周期门面：构造 Buffer、暴露身份、只读状态、保存点和 dirty 判断。
//!
//! 本文件只管理 Buffer 作为文档对象的外部可见状态，不执行具体编辑、坐标转换或历史回放。

use std::io;

use super::{Buffer, history};
use crate::{
    BufferConfig, BufferLoadError, BufferOrigin, BufferState, BufferVersion, EngineResult,
    LoadedTextInfo, TransactionId,
    storage::{RopeyStorage, TextRead, TextStorage},
    text_loading::streaming_decoder::{StreamDecodeError, decode_stream},
};

impl Buffer {
    /// 从已有文本创建匿名来源 Buffer。
    pub fn from_text(text: String, config: BufferConfig) -> EngineResult<Self> {
        Self::with_origin(BufferOrigin::anonymous(), text, config)
    }

    /// 从已有文本与指定来源创建 Buffer。
    ///
    /// `origin` 由宿主自由解释（可以是文件路径、URL、UUID 等任意不透明句柄）；
    /// 引擎只用它做相等性 / 哈希 / 展示，**不**做任何 I/O 或路径解析。
    pub fn with_origin(
        origin: BufferOrigin,
        text: String,
        config: BufferConfig,
    ) -> EngineResult<Self> {
        let storage = RopeyStorage::new(text);
        Ok(Self::from_parts(origin, storage, config, None))
    }

    /// 从 `io::Read` 流式加载 Buffer。
    ///
    /// 等价于 `with_origin(origin, text, config)` 的语义，但**单次**遍历完成
    /// UTF-8 校验、BOM 剥离、行尾风格识别、最长行字符数与末尾换行判定，并把
    /// 字节增量喂给 `ropey::RopeBuilder`——任一时刻内存只持有一份固定大小的
    /// 读缓冲（64 KiB）加上正在构建的 rope，不再出现 `bytes Vec` + `String`
    /// + `Rope` 三份全量内存同时活的局面。
    ///
    /// 64 MiB 文本经此路径加载 peak RSS 从 218 MB 降至 ~100 MB 量级。
    ///
    /// 失败情形：见 [`BufferLoadError`]。
    pub fn from_reader<R: io::Read>(
        origin: BufferOrigin,
        reader: R,
        config: BufferConfig,
    ) -> Result<Self, BufferLoadError> {
        let result = decode_stream(reader, &config).map_err(|e| match e {
            StreamDecodeError::Io(io_err) => BufferLoadError::Io(io_err),
            StreamDecodeError::Engine(engine_err) => BufferLoadError::Engine(engine_err),
        })?;
        let storage = RopeyStorage::from_rope(result.rope);
        let mut buffer = Self::from_parts(origin, storage, config, Some(result.info));
        buffer.mark_synced_external();
        Ok(buffer)
    }

    /// from_text / with_origin / from_reader 的共享构造路径。
    ///
    /// Buffer 字段默认值变更只需改这一处。
    fn from_parts(
        origin: BufferOrigin,
        storage: RopeyStorage,
        config: BufferConfig,
        loaded_text_info: Option<LoadedTextInfo>,
    ) -> Self {
        let saved_snapshot = storage.snapshot();
        let saved_fingerprint = saved_snapshot.fingerprint();
        let mut buffer = Self {
            origin,
            read_only: false,
            config,
            storage,
            version: BufferVersion::INITIAL,
            saved_version: BufferVersion::INITIAL,
            saved_snapshot,
            saved_fingerprint,
            last_synced_external_version: None,
            loaded_text_info,
            next_transaction_id: TransactionId::INITIAL,
            text_changes: Default::default(),
            history: history::HistoryState::new(),
            session: None,
        };
        buffer.apply_large_file_auto_read_only();
        buffer
    }

    /// 加载 / reload 后按 `LargeFilePolicy::auto_read_only_on_large_file`
    /// 决定是否切到只读；只在大文件触发时把 `read_only` 置为 `true`，
    /// 不会主动取消既有的只读状态。
    pub(in crate::buffer) fn apply_large_file_auto_read_only(&mut self) {
        if self.config.large_file.auto_read_only_on_large_file && self.is_large_file() {
            self.read_only = true;
        }
    }

    /// 语义等同 `from_text`。保留独立入口便于宿主语义清晰。
    pub fn scratch(text: String, config: BufferConfig) -> EngineResult<Self> {
        Self::with_origin(BufferOrigin::anonymous(), text, config)
    }

    /// Buffer 来源句柄（宿主自解释，引擎不解析）。
    pub fn origin(&self) -> &BufferOrigin {
        &self.origin
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn into_read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub fn state(&self) -> BufferState {
        if self.read_only {
            BufferState::ReadOnly
        } else if self.is_dirty() {
            BufferState::Dirty
        } else {
            BufferState::Clean
        }
    }

    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    pub fn set_config(&mut self, config: BufferConfig) {
        self.config = config;
        self.truncate_undo_history_to_budget();
    }

    /// 当前 Buffer 文本字节数是否被 `LargeFilePolicy::large_file_threshold_bytes`
    /// 视为大文件。
    ///
    /// 这是基于当前 storage 的实时判断，不等同于 `LoadedTextInfo::is_large`
    /// （后者是加载时刻快照）。`large_file_threshold_bytes == 0` 时永远返回 `false`。
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

    pub fn is_dirty(&self) -> bool {
        if self.version == self.saved_version {
            return false;
        }

        let current_fingerprint = self.storage.fingerprint();

        if current_fingerprint != self.saved_fingerprint {
            return true;
        }

        !self.storage.has_same_text(&self.saved_snapshot)
    }

    pub fn mark_saved(&mut self) {
        self.saved_version = self.version;
        self.saved_snapshot = self.storage.snapshot();
        self.saved_fingerprint = self.saved_snapshot.fingerprint();
    }

    pub fn mark_synced_external(&mut self) {
        self.last_synced_external_version = Some(self.version);
    }

    pub fn is_synced_with_external(&self) -> bool {
        self.last_synced_external_version == Some(self.version)
    }

    pub fn loaded_text_info(&self) -> Option<&LoadedTextInfo> {
        self.loaded_text_info.as_ref()
    }
}
