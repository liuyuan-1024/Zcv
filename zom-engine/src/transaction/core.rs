//! `Transaction`：绑定 base_version、EditList、metadata 和 selection 的提交请求。
//!
//! 这里不应用编辑；Buffer 的 transaction_pipeline 负责版本检查、原子提交和事件生成。
//! 提交后的事实快照见 `transaction_record::TransactionRecord`。

use crate::{
    EngineResult, errors::TransactionError, selection::SelectionSet, types::BufferVersion,
};

use super::{Edit, EditList, TransactionMetadata};

/// 批量编辑事务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    base_version: BufferVersion,
    edits: EditList,
    metadata: TransactionMetadata,
    before_selection: Option<SelectionSet>,
    after_selection: Option<SelectionSet>,
}

impl Transaction {
    pub fn new(base_version: BufferVersion, edits: EditList) -> Result<Self, TransactionError> {
        if edits.is_empty() {
            return Err(TransactionError::EmptyTransaction);
        }

        Ok(Self {
            base_version,
            edits,
            metadata: TransactionMetadata::default(),
            before_selection: None,
            after_selection: None,
        })
    }

    pub fn from_edits(base_version: BufferVersion, edits: Vec<Edit>) -> EngineResult<Self> {
        let edits = EditList::new(edits)?;
        Ok(Self::new(base_version, edits)?)
    }

    pub fn with_metadata(mut self, metadata: TransactionMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn with_selection(
        mut self,
        before_selection: Option<SelectionSet>,
        after_selection: Option<SelectionSet>,
    ) -> Self {
        self.before_selection = before_selection;
        self.after_selection = after_selection;
        self
    }

    pub fn base_version(&self) -> BufferVersion {
        self.base_version
    }

    pub fn edits(&self) -> &EditList {
        &self.edits
    }

    pub fn metadata(&self) -> &TransactionMetadata {
        &self.metadata
    }

    pub fn before_selection(&self) -> Option<&SelectionSet> {
        self.before_selection.as_ref()
    }

    pub fn after_selection(&self) -> Option<&SelectionSet> {
        self.after_selection.as_ref()
    }

    pub fn into_parts(
        self,
    ) -> (
        BufferVersion,
        EditList,
        TransactionMetadata,
        Option<SelectionSet>,
        Option<SelectionSet>,
    ) {
        (
            self.base_version,
            self.edits,
            self.metadata,
            self.before_selection,
            self.after_selection,
        )
    }
}
