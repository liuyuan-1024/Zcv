//! EditList：事务提交前的编辑排序和重叠校验边界。
//!
//! 它保证编辑互不重叠，但允许空列表；空事务由 Transaction 层拒绝。
//!
//! **Zero-copy 纪律**：内部存储为 `Arc<[Edit]>`，`Clone` 是 O(1) 引用计数递增；
//! 单个事务内相同 replacement 会归一到同一份 `Arc<str>`。

use std::{collections::HashMap, sync::Arc};

use crate::errors::EditError;

use super::{Edit, edit::empty_replacement};

/// 归一化且验证后的编辑列表。
///
/// 内部以 `Arc<[Edit]>` 存储；`Clone` / `apply_edit_list` 拷贝传递只递增引用计数。
/// 每个 `Edit` 的 replacement 也是 `Arc<str>`；`EditList::new` 会把同一事务内
/// 内容相同的 replacement 归一，避免多光标同文本编辑重复持有堆文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditList {
    edits: Arc<[Edit]>,
}

impl EditList {
    /// 创建并验证编辑列表，自动排序并检测重叠。
    ///
    /// 注意：这里允许空列表，因为「空事务」属于 Transaction 语义，
    /// 由 Transaction::new 拒绝。
    pub fn new(mut edits: Vec<Edit>) -> Result<Self, EditError> {
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

    pub fn len(&self) -> usize {
        self.edits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub fn as_slice(&self) -> &[Edit] {
        &self.edits
    }

    /// 物化为 `Vec<Edit>`。命名让分配语义显眼；事务热路径应使用 `as_slice` / iter。
    pub fn into_inner(self) -> Vec<Edit> {
        // Arc<[T]> 无法直接 try_unwrap；只能逐元素拷贝（每个 Edit 内部 payload 仍是 Arc clone）。
        // 大多数路径已不再调用 into_inner，残留路径接受这次一次性分配。
        self.edits.iter().cloned().collect()
    }
}

fn share_repeated_replacements(edits: &mut [Edit]) {
    if edits.len() <= 1 {
        return;
    }

    let mut interned: HashMap<Arc<str>, ()> = HashMap::with_capacity(edits.len());

    for edit in edits {
        if edit.replacement().is_empty() {
            edit.share_replacement_with(empty_replacement());
            continue;
        }

        let shared = interned
            .get_key_value(edit.replacement())
            .map(|(replacement, ())| Arc::clone(replacement));

        if let Some(shared) = shared {
            edit.share_replacement_with(shared);
        } else {
            interned.insert(Arc::clone(edit.replacement_arc()), ());
        }
    }
}
