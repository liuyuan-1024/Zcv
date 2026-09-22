//! 基线派生快照：在只读基线上规划编辑，并在版本校验后整体安装。
//!
//! 对齐 Zed 的 Buffer::snapshot_with_edits / fast_forward / EditedBufferSnapshot：
//! 规划阶段在克隆的存储、日志、坐标索引与历史上完成全部可失败步骤，不推进主文档；
//! 安装阶段在版本校验通过后整体换入派生状态，并发布订阅批次。

use super::Buffer;
use super::transaction_pipeline::prepared::DerivedBufferState;
use crate::{
    BufferVersion, Snapshot, TextResult,
    errors::TransactionError,
    transaction::{Edit, EditList, TransactionMetadata},
};

/// 在只读基线上规划编辑得到的派生状态，等待版本校验后安装。
pub struct EditedBufferSnapshot {
    base_version: BufferVersion,
    state: Option<DerivedBufferState>,
    snapshot: Snapshot,
    did_edit: bool,
}

impl EditedBufferSnapshot {
    /// 规划所基于的主文档版本；安装时主文档必须仍停在该版本。
    pub fn base_version(&self) -> BufferVersion {
        self.base_version
    }

    /// 派生快照：基线文本应用编辑后的不可变读取视图。
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// 本次规划是否实际应用了编辑。
    pub fn did_edit(&self) -> bool {
        self.did_edit
    }
}

impl Buffer {
    /// 在只读基线上规划编辑，返回等待版本校验的派生快照。
    ///
    /// 规划只读取当前 Buffer；主文档版本、订阅与历史都不变。安装由 `fast_forward` 完成。
    pub fn snapshot_with_edits<I>(&self, edits: I) -> TextResult<EditedBufferSnapshot>
    where
        I: IntoIterator<Item = Edit>,
    {
        let forward = EditList::new(edits.into_iter().collect())?;
        let base_version = self.version();
        if forward.is_empty() {
            return Ok(EditedBufferSnapshot {
                base_version,
                state: None,
                snapshot: self.snapshot(),
                did_edit: false,
            });
        }

        let state = self.plan_edit_list_with_metadata(
            base_version,
            forward,
            TransactionMetadata::default(),
        )?;
        let snapshot = Snapshot::new(
            state.storage.snapshot(),
            state.version,
            self.config().clone(),
            state.edit_log.clone(),
            state.coordinate_index.clone(),
        );
        Ok(EditedBufferSnapshot {
            base_version,
            state: Some(state),
            snapshot,
            did_edit: true,
        })
    }

    /// 主文档版本仍等于规划基线时，整体换入派生状态。
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
        if let Some(state) = edited.state {
            self.install(state);
        }
        Ok(())
    }
}
