//! Buffer selection 状态入口：读取、设置和通过 ChangeSet 映射当前 SelectionSet。
//!
//! 本文件只维护 selection 与 Buffer 边界校验的关系，不生成文本编辑，也不定义 selection 归一化算法本身。

use crate::{ChangeSet, EngineResult, SelectionSet};

use super::Buffer;

impl Buffer {
    pub fn selection(&self) -> &SelectionSet {
        &self.selection
    }

    pub fn set_selection(&mut self, selection: SelectionSet) -> EngineResult<()> {
        self.validate_selection_set(&selection)?;
        self.selection = selection;
        Ok(())
    }

    pub fn selection_after_edit(
        &self,
        selection: &SelectionSet,
        changeset: &ChangeSet,
    ) -> SelectionSet {
        selection.map_through_changeset(changeset)
    }
}
