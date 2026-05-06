//! Zom Engine - 底层文本编辑引擎
//!
//! 不直接负责 UI 渲染、LSP 协议、语法树生成或项目级索引。
//! 仅专注于文本存储、坐标模型、事务变异、历史系统以及外部系统所需的底层文本协作接口。

mod coordinates_core;
pub(crate) mod storage;

pub mod buffer;
pub mod config;
pub mod errors;
pub mod loading;
pub mod selection;
pub mod snapshot;
pub mod transaction;
pub mod types;

pub use buffer::{Buffer, HistoryStatus};
pub use config::{
    BufferConfig, DisplayColumnAffinity, DisplayWidthPolicy, EncodingConfig, LargeFilePolicy,
    LineEndingConfig, PositionEncodingConfig, TabConfig, WordBoundaryPolicy,
};
pub use errors::{
    CoordinateError, EditError, EngineError, EngineResult, StorageError, TransactionError,
};
pub use loading::{BomPolicy, InvalidUtf8Policy, LoadedTextInfo, TextEncoding};
pub use selection::{
    Affinity, CompositionSelection, CompositionState, Cursor, MovementDirection, MovementUnit,
    Selection, SelectionMergePolicy, SelectionSet,
};
pub use snapshot::Snapshot;
pub use transaction::{
    ChangeSet, Delta, Edit, EditList, Transaction, TransactionMergePolicy, TransactionMetadata,
    TransactionSource,
};
pub use types::{
    BufferId, BufferKind, BufferState, BufferVersion, ByteOffset, CharOffset, DisplayColumn, Line,
    LineEndingStyle, LogicalColumn, Position, TextRange, TransactionId, Utf16Offset, Utf16Position,
};
