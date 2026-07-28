//! 可编辑 Buffer 的 public 入口与状态聚合。
//!
//! `buffer` 模块按能力域拆分，`Buffer` 的 public API 不依赖这些目录路径：
//! - `versioning`：BufferVersion、低成本 Snapshot 创建与过期判断
//! - `coordinates`：byte / char / UTF-16 / line 坐标转换、grapheme 与 CRLF 边界
//! - `movement`：Word / Identifier / Subword / Symbol 移动
//! - `edit_ops`：单范围文本变异入口
//! - `history`：Undo / Redo、历史合并与本地分支
//! - `transaction_pipeline`：事务准备、提交和 history 收尾
//! - `validation`：Buffer 级边界校验

use crate::{
    BufferConfig, BufferId, BufferOrigin, BufferVersion, LoadedTextInfo, TransactionId,
    storage::{RopeySnapshot, RopeyStorage, TextFingerprint},
    transaction::DeltaEvent,
};

mod coordinates;
mod edit_ops;
mod events;
mod history;
mod lifecycle;
mod movement;
mod reload;
mod replace;
mod slicing;
mod transaction_pipeline;
mod validation;
mod versioning;

pub use history::{HistoryEditOutcome, HistoryNodeId, HistoryNodeView, HistoryStatus};

/// 最小可编辑 Buffer。
#[derive(Debug, Clone)]
pub struct Buffer {
    id: BufferId,
    origin: BufferOrigin,
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
}
