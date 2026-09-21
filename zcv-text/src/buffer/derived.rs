//! 基线派生快照：在稳定基线上应用编辑得到派生快照，并在版本校验后原子安装。
//!
//! 对齐 Zed 的 `Buffer::snapshot_with_edits` / `Buffer::fast_forward` / `EditedBufferSnapshot`：
//! 派生只读取当前 Buffer，在存储副本上应用编辑，不推进版本、不发布订阅、不写历史；
//! 安装校验主文档版本未前进，过期结果显式拒绝，并通过正常事务路径推进版本、订阅与历史。

use super::Buffer;
use crate::{
    BufferVersion, Snapshot, TextResult, TransactionMetadata,
    errors::{TextError, TransactionError},
    text_changes::TextPatch,
    transaction::{Edit, EditList},
};

/// 在基线快照副本上应用编辑得到的派生快照，等待版本校验后安装。
#[derive(Debug)]
pub struct EditedBufferSnapshot {
    base_version: BufferVersion,
    snapshot: Snapshot,
    forward: EditList,
    did_edit: bool,
}

impl EditedBufferSnapshot {
    /// 派生所基于的主文档版本；安装时主文档必须仍停在该版本。
    pub fn base_version(&self) -> BufferVersion {
        self.base_version
    }

    /// 派生快照：基线文本应用编辑后的不可变读取视图，版本为基线的下一个版本。
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// 本次派生是否实际应用了编辑。
    pub fn did_edit(&self) -> bool {
        self.did_edit
    }
}

impl Buffer {
    /// 在基线快照副本上应用编辑，返回等待版本校验的派生快照。
    ///
    /// 派生只读取当前 Buffer；主文档版本、订阅与历史都不变。安装由 `fast_forward` 完成。
    pub fn snapshot_with_edits<I>(&self, edits: I) -> TextResult<EditedBufferSnapshot>
    where
        I: IntoIterator<Item = Edit>,
    {
        let forward = EditList::new(edits.into_iter().collect())?;
        let base_version = self.version();
        if forward.is_empty() {
            return Ok(EditedBufferSnapshot {
                base_version,
                snapshot: self.snapshot(),
                forward,
                did_edit: false,
            });
        }

        self.validate_edit_list(&forward)?;
        let new_version = base_version.next().ok_or(TextError::VersionOverflow)?;
        let mut storage = self.storage.clone();
        storage.apply_edit_list(&forward)?;
        let undo = self.build_inverse_edit_list(&forward)?;
        let patch = TextPatch::from_edit_list(forward.as_slice());
        let snapshot = Snapshot::new(
            storage.snapshot(),
            new_version,
            self.config().clone(),
            self.edit_log
                .appended(base_version, new_version, forward.clone(), Some(undo)),
            self.coordinate_index
                .appended(base_version, new_version, patch),
        );

        Ok(EditedBufferSnapshot {
            base_version,
            snapshot,
            forward,
            did_edit: true,
        })
    }

    /// 主文档版本仍等于派生基线时，通过正常事务路径安装派生快照。
    ///
    /// 版本已前进（含任何其他事务推进）时显式拒绝，调用方必须丢弃过期结果。
    pub fn fast_forward(&mut self, edited: EditedBufferSnapshot) -> TextResult<()> {
        self.ensure_writable()?;
        if edited.base_version != self.version() {
            return Err(TransactionError::VersionMismatch {
                expected: self.version(),
                actual: edited.base_version,
            }
            .into());
        }
        if !edited.did_edit {
            return Ok(());
        }

        self.edit(
            edited.forward.as_slice().to_vec(),
            TransactionMetadata::default(),
        )
        .map(|_| ())
    }
}
