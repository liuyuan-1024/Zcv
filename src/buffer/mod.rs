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

use crate::{
    BufferConfig, BufferId, BufferKind, BufferVersion, CompositionState, LoadedTextInfo,
    SelectionSet, TransactionId,
    storage::{RopeySnapshot, RopeyStorage, TextFingerprint},
    transaction::DeltaEvent,
};

mod composition;
pub(crate) mod coordinates;
mod edit_ops;
mod events;
mod history;
mod lifecycle;
mod loading;
mod movement;
mod reload;
mod selection_ops;
mod slicing;
mod transaction_pipeline;
mod validation;
mod versioning;

pub use history::HistoryStatus;

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
    saved_snapshot: RopeySnapshot,
    saved_fingerprint: TextFingerprint,
    last_synced_external_version: Option<BufferVersion>,
    loaded_text_info: Option<LoadedTextInfo>,
    next_transaction_id: TransactionId,
    pending_delta_events: Vec<DeltaEvent>,
    last_delta_event: Option<DeltaEvent>,
    history: history::HistoryState,
    selection: SelectionSet,
    composition: Option<CompositionState>,
}
