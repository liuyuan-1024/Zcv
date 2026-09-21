//! Zcv 的纯文本内核。
//! 此文件是 `zcv-text` crate 的公共入口。
//!
//! 不直接负责 UI 渲染、文件 IO、LSP 协议、语法树生成或项目级索引。
//! 仅专注于文本存储、坐标模型、事务变异、历史系统与只读快照。

mod buffer;
mod config;
mod diff;
mod errors;
mod movement;
mod position_map;
mod slicing;
mod snapshot;
mod storage;
mod text_changes;
mod tracking;
mod transaction;
mod types;
pub mod word_diff;

pub use buffer::{Buffer, EditedBufferSnapshot, HistoryEditOutcome, movement_boundary_in_text};
pub use config::{BufferConfig, LargeFilePolicy, LargeTransactionPolicy, WordBoundaryPolicy};
pub use errors::{
    AnchorError, CoordinateError, EditError, StorageError, TextError, TextResult, TransactionError,
};
pub use movement::{MovementDirection, MovementUnit};
pub use position_map::{Affinity, MappingResult, PositionMap, Stickiness};
pub use slicing::{LineContent, LineSlice, TextSlice};
pub use snapshot::Snapshot;
pub use storage::TextRead;
pub use text_changes::{PatchEdit, TextChangeBatch, TextPatch, TextSubscription};
pub use tracking::Anchor;
pub use transaction::{
    ChangeSet, Delta, DeltaEvent, Edit, TransactionMergePolicy, TransactionMetadata,
    TransactionOutcome, TransactionSource,
};
pub use types::{
    BufferId, BufferVersion, ByteOffset, CharOffset, Line, LineEndingStyle, LineRange,
    LogicalColumn, Position, TextRange, TransactionId, Utf16Offset, Utf16Position,
};
