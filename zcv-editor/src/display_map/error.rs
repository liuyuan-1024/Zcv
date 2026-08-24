use thiserror::Error;
use zcv_text::{CoordinateError, TextError, TextRange};

pub(crate) type DisplayMapResult<T> = Result<T, DisplayMapError>;

#[derive(Debug, Error)]
pub(crate) enum DisplayMapError {
    #[error(transparent)]
    Text(#[from] TextError),
    #[error(transparent)]
    Coordinate(#[from] CoordinateError),
    #[error(transparent)]
    Fold(#[from] FoldError),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum FoldError {
    #[error("FoldMap fold id 溢出")]
    IdOverflow,
    #[error("折叠区间与已有折叠部分重叠：已有 {existing:?}，候选 {candidate:?}")]
    OverlapWithoutNesting {
        existing: TextRange,
        candidate: TextRange,
    },
    #[error("折叠区间不能为空：{range:?}")]
    EmptyRange { range: TextRange },
}
