//! Buffer 生命周期门面：构造 Buffer、暴露身份、只读状态、保存点和 dirty 判断。
//!
//! 本文件只管理 Buffer 作为文档对象的外部可见状态，不执行具体编辑、坐标转换或历史回放。

use std::{
    borrow::Cow,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    BufferConfig, BufferId, BufferKind, BufferState, BufferVersion, EngineResult, LoadedTextInfo,
    SelectionSet, TransactionId,
    storage::{RopeyStorage, TextRead, TextStorage},
};

use super::{Buffer, history};

static NEXT_BUFFER_ID: AtomicU64 = AtomicU64::new(1);

impl Buffer {
    /// 创建空 Buffer。
    pub fn new(config: BufferConfig) -> EngineResult<Self> {
        Self::from_text(String::new(), config)
    }

    /// 从已有文本创建 Buffer。
    pub fn from_text(text: String, config: BufferConfig) -> EngineResult<Self> {
        Self::from_kind_text(BufferKind::Untitled, text, config)
    }

    /// 从已有文本创建带明确生命周期类型的 Buffer。
    pub fn from_kind_text(
        kind: BufferKind,
        text: String,
        config: BufferConfig,
    ) -> EngineResult<Self> {
        let storage = RopeyStorage::new(text);
        let saved_snapshot = storage.snapshot();
        let saved_fingerprint = saved_snapshot.fingerprint();

        let mut buffer = Self {
            id: next_buffer_id(),
            kind,
            read_only: false,
            config,
            storage,
            version: BufferVersion::INITIAL,
            saved_version: BufferVersion::INITIAL,
            last_saved_version: BufferVersion::INITIAL,
            saved_snapshot,
            saved_fingerprint,
            last_synced_external_version: None,
            loaded_text_info: None,
            next_transaction_id: TransactionId::INITIAL,
            pending_delta_events: Vec::new(),
            last_delta_event: None,
            history: history::HistoryState::new(),
            selection: SelectionSet::default(),
            composition: None,
        };
        buffer.apply_large_file_auto_read_only();
        Ok(buffer)
    }

    /// 加载 / reload 后按 `LargeFilePolicy::auto_read_only_on_large_file`
    /// 决定是否切到只读；只在大文件触发时把 `read_only` 置为 `true`，
    /// 不会主动取消既有的只读状态。
    pub(in crate::buffer) fn apply_large_file_auto_read_only(&mut self) {
        if self.config.large_file.auto_read_only_on_large_file && self.is_large_file() {
            self.read_only = true;
        }
    }

    /// 从文件文本创建 Buffer。文件读取、编码探测和保存输出由宿主或后续阶段负责。
    pub fn from_file_text(
        path: impl Into<std::path::PathBuf>,
        text: String,
        config: BufferConfig,
    ) -> EngineResult<Self> {
        Self::from_kind_text(BufferKind::file(path), text, config)
    }

    /// 从 URI 绑定文本创建 Buffer。URI 只作为身份信息保存，不触发任何 I/O。
    pub fn from_uri_text(
        uri: impl Into<String>,
        text: String,
        config: BufferConfig,
    ) -> EngineResult<Self> {
        Self::from_kind_text(BufferKind::uri(uri), text, config)
    }

    /// 创建临时草稿 Buffer。
    pub fn scratch(text: String, config: BufferConfig) -> EngineResult<Self> {
        Self::from_kind_text(BufferKind::Scratch, text, config)
    }

    pub fn id(&self) -> BufferId {
        self.id
    }

    pub fn kind(&self) -> &BufferKind {
        &self.kind
    }

    pub fn path(&self) -> Option<&Path> {
        self.kind.path()
    }

    pub fn uri(&self) -> Option<&str> {
        self.kind.uri_str()
    }

    pub fn is_temporary(&self) -> bool {
        self.kind.is_temporary()
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
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

    pub fn has_unsaved_changes(&self) -> bool {
        self.is_dirty()
    }

    pub fn can_close_without_prompt(&self) -> bool {
        !self.has_unsaved_changes()
    }

    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    /// 返回全文。
    ///
    /// 返回 Cow 而不是 `&str`，public API 不承诺全文连续内存。
    /// 热路径请优先用 Snapshot / slice / line API。
    pub fn text(&self) -> Cow<'_, str> {
        self.storage.text()
    }

    pub fn len_chars(&self) -> crate::CharOffset {
        self.storage.len_chars()
    }

    pub fn len_bytes(&self) -> usize {
        self.storage.len_bytes()
    }

    pub fn len_utf16_cu(&self) -> usize {
        self.storage.len_utf16_cu()
    }

    /// 当前 Buffer 文本字节数是否被 `LargeFilePolicy::large_file_threshold_bytes`
    /// 视为大文件。
    ///
    /// 这是基于当前 storage 的实时判断，不等同于 `LoadedTextInfo::is_large`
    /// （后者是加载时刻快照）。`large_file_threshold_bytes == 0` 时永远返回 `false`。
    pub fn is_large_file(&self) -> bool {
        self.config
            .large_file
            .is_large_byte_size(self.storage.len_bytes())
    }

    /// 当前 Buffer 是否含有按 `LargeFilePolicy::long_line_threshold_chars`
    /// 视为超长的行。
    ///
    /// O(N) 扫描；调用方应自行决定调用频率。`long_line_threshold_chars == 0`
    /// 时永远返回 `false`。
    pub fn has_long_line(&self) -> bool {
        self.config
            .large_file
            .is_long_line(self.longest_line_chars())
    }

    /// 当前 Buffer 中最长一行的字符数（不含行尾换行符）。
    ///
    /// 通过遍历当前文本计算，不缓存；O(N) 扫描。
    pub fn longest_line_chars(&self) -> usize {
        super::loading::longest_line_chars_in(self.text().as_ref())
    }

    /// 当前 Buffer 大致内存占用估算（字节）。
    ///
    /// 度量包含：
    /// - 文本存储字节数（`len_bytes()`，不计 `ropey::Rope` 内部节点开销）
    /// - 历史图按 `HistoryStatus::memory_bytes` 累加的字符串占用
    /// - selection / pending DeltaEvent 队列的固定大小估算
    ///
    /// 仅作为粗估指标，用于宿主侧的内存观测与回归监控；不承诺等同于操作系统
    /// 实际驻留集 (RSS) 或 `ropey` 内部节点 / 缓存的精确字节数。
    pub fn approximate_memory_bytes(&self) -> usize {
        let text_bytes = self.storage.len_bytes();
        let history_bytes = self.history.status().memory_bytes;
        let selection_bytes =
            self.selection.as_slice().len() * std::mem::size_of::<crate::Selection>();
        let pending_events =
            self.pending_delta_events.len() * std::mem::size_of::<crate::transaction::DeltaEvent>();
        text_bytes
            .saturating_add(history_bytes)
            .saturating_add(selection_bytes)
            .saturating_add(pending_events)
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn saved_version(&self) -> BufferVersion {
        self.saved_version
    }

    pub fn last_saved_version(&self) -> BufferVersion {
        self.last_saved_version
    }

    pub fn last_synced_external_version(&self) -> Option<BufferVersion> {
        self.last_synced_external_version
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
        self.last_saved_version = self.version;
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

fn next_buffer_id() -> BufferId {
    BufferId::new(NEXT_BUFFER_ID.fetch_add(1, Ordering::Relaxed))
}
