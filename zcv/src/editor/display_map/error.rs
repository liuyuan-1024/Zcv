use thiserror::Error;
use zcv_engine::{BufferVersion, CoordinateError, EngineError, TextRange};

pub(crate) type DisplayMapResult<T> = Result<T, DisplayMapError>;

#[derive(Debug, Error)]
pub(crate) enum DisplayMapError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Coordinate(#[from] CoordinateError),
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Projection(#[from] ProjectionError),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum FoldError {
    #[error("FoldSet fold range id 溢出")]
    IdOverflow,
    #[error("FoldSet 版本不匹配：预期版本 {expected:?}，实际版本 {actual:?}")]
    VersionMismatch {
        expected: BufferVersion,
        actual: BufferVersion,
    },
    #[error("折叠区间与已有折叠部分重叠：已有 {existing:?}，候选 {candidate:?}")]
    OverlapWithoutNesting {
        existing: TextRange,
        candidate: TextRange,
    },
    #[error("折叠区间不能为空：{range:?}")]
    EmptyRange { range: TextRange },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum ProjectionError {
    #[error(
        "Projection 版本不匹配：snapshot 版本 {snapshot_version:?}，fold 版本 {fold_version:?}"
    )]
    VersionMismatch {
        snapshot_version: BufferVersion,
        fold_version: BufferVersion,
    },
    #[error(
        "Projection::apply_patch 版本不匹配：projection {projection_version:?}，patch old/new {patch_old_version:?}/{patch_new_version:?}，snapshot {snapshot_version:?}，folds {fold_version:?}"
    )]
    ApplyPatchStale {
        projection_version: BufferVersion,
        patch_old_version: BufferVersion,
        patch_new_version: BufferVersion,
        snapshot_version: BufferVersion,
        fold_version: BufferVersion,
    },
}
