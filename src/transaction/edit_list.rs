//! EditList：事务提交前的编辑排序和重叠校验边界。
//!
//! 它保证编辑互不重叠，但允许空列表；空事务由 Transaction 层拒绝。

use crate::errors::EditError;

use super::Edit;

/// 归一化且验证后的编辑列表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditList {
    edits: Vec<Edit>,
}

impl EditList {
    /// 创建并验证编辑列表，自动排序并检测重叠。
    ///
    /// 注意：这里允许空列表，因为“空事务”属于 Transaction 语义，
    /// 由 Transaction::new 拒绝。
    pub fn new(mut edits: Vec<Edit>) -> Result<Self, EditError> {
        edits.sort_by_key(|edit| edit.range.start());

        for i in 1..edits.len() {
            let previous = &edits[i - 1];
            let current = &edits[i];

            if previous.range.end() > current.range.start() {
                return Err(EditError::OverlappingEdits {
                    previous: previous.range,
                    current: current.range,
                });
            }
        }

        Ok(Self { edits })
    }

    pub fn len(&self) -> usize {
        self.edits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub fn as_slice(&self) -> &[Edit] {
        &self.edits
    }

    pub fn into_inner(self) -> Vec<Edit> {
        self.edits
    }
}
