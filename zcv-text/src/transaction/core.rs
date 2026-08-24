//! `Transaction`：绑定 base_version、EditList 和 metadata 的文本提交请求。
//!
//! 这里不应用编辑；Buffer 的 transaction_pipeline 负责版本检查、原子提交和事件生成。

use super::{Edit, EditList, TransactionMetadata};
use crate::{TextResult, errors::TransactionError, types::BufferVersion};

/// 批量编辑事务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Transaction {
    base_version: BufferVersion,
    edits: EditList,
    metadata: TransactionMetadata,
}

impl Transaction {
    pub(crate) fn new(
        base_version: BufferVersion,
        edits: EditList,
    ) -> Result<Self, TransactionError> {
        if edits.is_empty() {
            return Err(TransactionError::EmptyTransaction);
        }

        Ok(Self {
            base_version,
            edits,
            metadata: TransactionMetadata::default(),
        })
    }

    pub(crate) fn from_edits(base_version: BufferVersion, edits: Vec<Edit>) -> TextResult<Self> {
        let edits = EditList::new(edits)?;
        Ok(Self::new(base_version, edits)?)
    }

    pub(crate) fn with_metadata(mut self, metadata: TransactionMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub(crate) fn into_parts(self) -> (BufferVersion, EditList, TransactionMetadata) {
        (self.base_version, self.edits, self.metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BufferVersion, TransactionError, transaction::EditList};

    #[test]
    fn transaction_empty_edit_list_should_be_rejected_before_state_transition() {
        let err = Transaction::new(BufferVersion::INITIAL, EditList::new(Vec::new()).unwrap())
            .unwrap_err();

        assert_eq!(err, TransactionError::EmptyTransaction);
    }
}
