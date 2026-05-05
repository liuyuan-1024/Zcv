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

use std::borrow::Cow;

use crate::{
    BufferConfig, BufferVersion, CompositionState, EngineResult, SelectionSet,
    storage::{RopeyStorage, TextRead},
};

mod composition;
pub(crate) mod coordinates;
mod edit_ops;
mod history;
mod movement;
mod selection_ops;
mod transaction_pipeline;
mod validation;
mod versioning;

pub use history::HistoryStatus;

/// 最小可编辑 Buffer。
#[derive(Debug, Clone)]
pub struct Buffer {
    config: BufferConfig,
    storage: RopeyStorage,
    version: BufferVersion,
    saved_version: BufferVersion,
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
        Ok(Self {
            config,
            storage: RopeyStorage::new(text),
            version: BufferVersion::INITIAL,
            saved_version: BufferVersion::INITIAL,
            history: history::HistoryState::new(),
            selection: SelectionSet::default(),
            composition: None,
        })
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

    pub fn is_dirty(&self) -> bool {
        self.version != self.saved_version
    }

    pub fn mark_saved(&mut self) {
        self.saved_version = self.version;
    }
}
