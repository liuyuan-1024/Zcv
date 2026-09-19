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
    BufferConfig, BufferGeneration, BufferVersion, TransactionId,
    storage::RopeyStorage,
    text_changes::TextChangeTopic,
    tracking::{CoordinateIndex, EditLog},
};

mod coordinates;
mod edit_ops;
mod events;
mod history;
mod lifecycle;
mod movement;
mod reset;
mod slicing;
mod transaction_pipeline;
mod validation;
mod versioning;

pub use movement::movement_boundary_in_text;
pub(crate) use movement::{is_inside_word_in_text, surrounding_word_in_text};

pub use history::HistoryEditOutcome;

/// 最小可编辑 Buffer。
#[derive(Debug)]
pub struct Buffer {
    read_only: bool,
    config: BufferConfig,
    storage: RopeyStorage,
    version: BufferVersion,
    /// 当前内容代际；reset / 基线替换后开启新代际。
    generation: BufferGeneration,
    saved_version: BufferVersion,
    next_transaction_id: TransactionId,
    text_changes: TextChangeTopic,
    /// 唯一的版本化编辑事实：Snapshot 据此重建净编辑，History 据此回放 undo/redo。
    edit_log: EditLog,
    /// 不随编辑日志预算裁剪的版本坐标索引：Anchor 解析与跨版本坐标映射的唯一事实。
    coordinate_index: CoordinateIndex,
    history: history::HistoryState,
    /// 进行中的编辑会话（`start_transaction` 开启，`end_transaction` 提交）。
    session: Option<history::TransactionSession>,
}
