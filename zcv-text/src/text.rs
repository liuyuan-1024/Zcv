//! Zcv 的纯文本内核。
//! 此文件是 `zcv-text` crate 的公共入口。
//!
//! 不直接负责 UI 渲染、LSP 协议、语法树生成或项目级索引。
//! 仅专注于文本存储、坐标模型、事务变异、历史系统以及外部系统所需的底层文本协作接口。

mod buffer;
mod config;
mod diff;
mod errors;
mod movement;
mod position_map;
mod search;
mod slicing;
mod snapshot;
mod storage;
mod text_changes;
mod text_loading;
mod tracking;
mod transaction;
mod types;
mod versioned;

pub use buffer::{Buffer, HistoryEditOutcome, HistoryNodeId, HistoryNodeView, HistoryStatus};
pub use config::{
    BufferConfig, EncodingConfig, LargeFilePolicy, LargeTransactionPolicy, LineEndingConfig,
    TabConfig, WordBoundaryPolicy,
};
pub use errors::{
    AnchorError, BufferLoadError, BufferSaveError, CoordinateError, EditError, SearchError,
    StorageError, TextError, TextResult, TransactionError, VersionedResultError,
};
pub use movement::{MovementDirection, MovementUnit};
pub use position_map::{Affinity, Bias, MappingResult, PositionMap, Stickiness};
pub use search::{RegexSearchOptions, RegexSearchResult, SearchMatch, SearchOptions, SearchResult};
pub use slicing::{LineContent, LineSlice, TextSlice};
pub use snapshot::Snapshot;
pub use text_changes::{PatchEdit, TextChangeBatch, TextPatch, TextSubscription};
pub use text_loading::{BomPolicy, InvalidUtf8Policy};
pub use tracking::{
    Anchor, Mark, TrackedRange, TrackedRangeCollapsePolicy, TrackedRangeInvalidationPolicy,
    TrackedRangeUpdate, TrackedRangeUpdatePolicy,
};
pub use transaction::{
    ChangeSet, Delta, DeltaEvent, Edit, TransactionMergePolicy, TransactionMetadata,
    TransactionOutcome, TransactionSource,
};
pub use types::{
    BufferVersion, ByteOffset, CharOffset, Line, LineEndingStyle, LineRange, LogicalColumn,
    Position, TextRange, TransactionId, Utf16Offset, Utf16Position,
};
pub use versioned::VersionedResult;
