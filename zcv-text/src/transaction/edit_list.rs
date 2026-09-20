//! EditList：事务提交前的编辑排序和重叠校验边界。
//!
//! 它保证编辑互不重叠，但允许空列表；空事务由 Transaction 层拒绝。
//!
//! **Zero-copy 纪律**：内部存储为 `Arc<[Edit]>`，`Clone` 是 O(1) 引用计数递增；
//! 单个事务内相同 replacement 会归一到同一份 `Arc<str>`。

use std::{collections::HashMap, sync::Arc};

use super::{Edit, edit::empty_replacement};
use crate::errors::EditError;

/// 归一化且验证后的编辑列表。
///
/// 内部以 `Arc<[Edit]>` 存储；`Clone` / `apply_edit_list` 拷贝传递只递增引用计数。
/// 每个 `Edit` 的 replacement 也是 `Arc<str>`；`EditList::new` 会把同一事务内内容相同的 replacement 归一，避免多光标同文本编辑重复持有堆文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditList {
    edits: Arc<[Edit]>,
}

impl EditList {
    /// 创建并验证编辑列表，自动排序并检测重叠。
    ///
    /// 注意：这里允许空列表，因为「空事务」属于 Transaction 语义，由 Transaction::new 拒绝。
    pub(crate) fn new(mut edits: Vec<Edit>) -> Result<Self, EditError> {
        edits.sort_by_key(|edit| edit.range().start());

        for i in 1..edits.len() {
            let previous = &edits[i - 1];
            let current = &edits[i];

            if previous.range().end() > current.range().start() {
                return Err(EditError::OverlappingEdits {
                    previous: previous.range(),
                    current: current.range(),
                });
            }
        }

        share_repeated_replacements(&mut edits);

        Ok(Self {
            edits: Arc::from(edits),
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.edits.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub(crate) fn as_slice(&self) -> &[Edit] {
        &self.edits
    }

    /// 本列表内所有 `Edit::replacement` 的 UTF-8 字节和。
    pub(crate) fn replacement_bytes(&self) -> usize {
        self.edits.iter().map(|edit| edit.replacement().len()).sum()
    }
}

fn share_repeated_replacements(edits: &mut [Edit]) {
    if edits.len() <= 1 {
        return;
    }

    // &str → Arc<str>：按值查找已有 replacement，命中则共享 Arc，未命中则登记当前 Arc。
    let mut interned: HashMap<&str, Arc<str>> = HashMap::with_capacity(edits.len());

    for edit in edits {
        if edit.replacement().is_empty() {
            edit.share_replacement_with(empty_replacement());
            continue;
        }

        if let Some(canonical) = interned.get(edit.replacement()) {
            edit.share_replacement_with(Arc::clone(canonical));
        } else {
            interned.insert(edit.replacement(), Arc::clone(edit.replacement_arc()));
        }
    }
}

#[cfg(test)]
#[path = "test/edit_list_tests.rs"]
mod tests;
