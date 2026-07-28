//! zcv Engine - 底层文本编辑引擎
//!
//! 不直接负责 UI 渲染、LSP 协议、语法树生成或项目级索引。
//! 仅专注于文本存储、坐标模型、事务变异、历史系统以及外部系统所需的底层文本协作接口。

mod buffer;
mod config;
mod coordinates;
mod errors;
mod position_map;
mod search;
mod selection;
mod slicing;
mod snapshot;
mod storage;
mod text_loading;
mod tracking;
mod transaction;
mod types;
mod versioned;

pub use buffer::{Buffer, HistoryEditOutcome, HistoryNodeId, HistoryNodeView, HistoryStatus};
pub use config::{
    BufferConfig, DisplayColumnAffinity, DisplayWidthPolicy, EncodingConfig, LargeFilePolicy,
    LargeTransactionPolicy, LineEndingConfig, PositionEncodingConfig, TabConfig,
    WordBoundaryPolicy,
};
pub use errors::{
    AnchorError, BufferLoadError, BufferSaveError, CoordinateError, EditError, EngineError,
    EngineResult, SearchError, StorageError, TransactionError, VersionedResultError,
};
pub use position_map::{Affinity, Bias, MappingResult, PositionMap, Stickiness};
pub use search::{RegexSearchOptions, RegexSearchResult, SearchMatch, SearchOptions, SearchResult};
pub use selection::{
    Cursor, MovementDirection, MovementUnit, Selection, SelectionMergePolicy, SelectionSet,
};
pub use slicing::{LineSlice, TextSlice, Viewport, ViewportSlice, VisibleLine};
pub use snapshot::Snapshot;
pub use text_loading::{BomPolicy, InvalidUtf8Policy, LoadedTextInfo, TextEncoding};
pub use tracking::{
    Anchor, AnchorDeletedPolicy, AnchorUpdate, Mark, TrackedRange, TrackedRangeCollapsePolicy,
    TrackedRangeInvalidationPolicy, TrackedRangeUpdate, TrackedRangeUpdatePolicy,
};
pub use transaction::{
    ChangeSet, Delta, DeltaEvent, Edit, EditList, Transaction, TransactionMergePolicy,
    TransactionMetadata, TransactionOutcome, TransactionRecord, TransactionSource,
};
pub(crate) use types::BufferId;
pub use types::{
    BufferOrigin, BufferState, BufferVersion, ByteOffset, CharOffset, DisplayColumn, Line,
    LineEndingStyle, LineRange, LogicalColumn, OriginKind, Position, TextRange, TransactionId,
    Utf16Offset, Utf16Position,
};
pub use versioned::{
    VersionedRangeEntry, VersionedRangeEntryId, VersionedRangeSet, VersionedRangeSpec,
    VersionedRangeUpdate, VersionedResult,
};
