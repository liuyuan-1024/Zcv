//! 可编辑 Buffer 的 public 入口与状态聚合。
//!
//! `buffer` 模块按能力域拆分：
//! - `versioning`：BufferVersion、低成本 Snapshot 创建与过期判断
//! - `coordinates`：坐标转换、grapheme、CRLF、DisplayColumn 数学
//! - `selection_ops`：SelectionSet 状态管理
//! - `movement`：M6B Word / Identifier / Subword / Symbol 移动
//! - `composition`：M6C IME composition 生命周期
//! - `edit_ops`：文本变异、多光标编辑入口
//! - `history`：Undo / Redo 与历史合并
//! - `transaction_pipeline`：事务准备、提交、selection 映射、history 收尾
//! - `validation`：Buffer 级边界校验
//!
//! 这样 `Buffer` 的 public API 保持稳定，但实现不再集中在一个超大文件里。

use std::{
    borrow::Cow,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    BufferConfig, BufferId, BufferKind, BufferState, BufferVersion, CompositionState, EngineResult,
    LoadedTextInfo, SelectionSet,
    storage::{RopeyStorage, TextRead},
};

mod composition;
pub(crate) mod coordinates;
mod edit_ops;
mod history;
mod loading;
mod movement;
mod selection_ops;
mod transaction_pipeline;
mod validation;
mod versioning;

pub use history::HistoryStatus;

static NEXT_BUFFER_ID: AtomicU64 = AtomicU64::new(1);

/// 最小可编辑 Buffer。
#[derive(Debug, Clone)]
pub struct Buffer {
    id: BufferId,
    kind: BufferKind,
    read_only: bool,
    config: BufferConfig,
    storage: RopeyStorage,
    version: BufferVersion,
    saved_version: BufferVersion,
    last_saved_version: BufferVersion,
    saved_text: String,
    last_synced_external_version: Option<BufferVersion>,
    loaded_text_info: Option<LoadedTextInfo>,
    history: history::HistoryState,
    selection: SelectionSet,
    composition: Option<CompositionState>,
}

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
        let saved_text = text.clone();

        Ok(Self {
            id: next_buffer_id(),
            kind,
            read_only: false,
            config,
            storage: RopeyStorage::new(text),
            version: BufferVersion::INITIAL,
            saved_version: BufferVersion::INITIAL,
            last_saved_version: BufferVersion::INITIAL,
            saved_text,
            last_synced_external_version: None,
            loaded_text_info: None,
            history: history::HistoryState::new(),
            selection: SelectionSet::default(),
            composition: None,
        })
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
    /// M4 后该方法返回 Cow，而不是 `&str`，避免 public API 继续承诺全文连续内存。
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
        self.text().as_ref() != self.saved_text
    }

    pub fn mark_saved(&mut self) {
        self.saved_version = self.version;
        self.last_saved_version = self.version;
        self.saved_text = self.text().into_owned();
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
