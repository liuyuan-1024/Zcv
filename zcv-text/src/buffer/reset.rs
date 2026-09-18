//! 内存基线重置：用外部文本整体替换 Buffer 内容、清理历史并刷新保存点。
//!
//! 文件 IO、编码恢复与保存策略属于 `zcv-project` 的文件边界；本模块只接收已解码的 `String`。

use super::Buffer;
use crate::{
    ByteOffset, TextRange, TextResult,
    diff::diff_patch,
    storage::{RopeyStorage, TextRead},
    transaction::{Edit, EditList, TransactionSource},
};

impl Buffer {
    /// 用外部文本整体重置 Buffer，使其成为新的干净基线。
    ///
    /// 文本变化时重建存储、递增版本并清空 history；文本相同时只推进保存点，保留现有版本和 history。
    /// 两种情况都会把 dirty 状态恢复为 clean，并向订阅发布旧文本 -> 新文本的 reset 事件，
    /// 使选区 / 折叠端点跟随外部变更后的具体位置。
    pub fn reset(&mut self, text: String) -> TextResult<()> {
        let old_version = self.version;
        // 替换存储前取出旧文本，生成真实的坐标映射 patch。
        let old_text = self.storage.slice_to_string(
            TextRange::new(ByteOffset::ZERO, self.storage.len_bytes())
                .expect("全文范围必须满足 start <= end"),
        )?;
        if old_text == text {
            self.mark_saved();
            return Ok(());
        }
        let patch = diff_patch(&old_text, &text);
        let edits = EditList::new(
            patch
                .edits()
                .iter()
                .map(|edit| {
                    Edit::replace(
                        edit.old_range(),
                        &text[edit.new_range().start().get()..edit.new_range().end().get()],
                    )
                })
                .collect(),
        )?;
        let new_storage = RopeyStorage::new(text);
        let (next_transaction_id, event) = self.prepare_delta_event(
            old_version,
            edits.clone(),
            TransactionSource::Programmatic,
            true,
        )?;
        // reset 会清空历史，因此不需要保留逆编辑。
        self.commit_prepared_text_change(new_storage, edits, None, next_transaction_id, &event);
        self.history.clear();
        self.session = None;
        self.mark_clean_internal();
        self.truncate_edit_history_to_budget();
        self.apply_large_file_auto_read_only();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BufferConfig, BufferVersion};

    #[test]
    fn reset_marks_text_changes_as_a_reset() {
        let mut buffer = Buffer::from_text("before\n".to_owned(), BufferConfig::default()).unwrap();
        let subscription = buffer.subscribe();

        buffer.reset("after\n".to_owned()).unwrap();

        let changes = subscription.consume();
        assert!(changes.requires_reset());
        assert!(changes.transaction_id().is_some());
        assert_eq!(changes.old_version(), Some(BufferVersion::INITIAL));
        assert_eq!(changes.new_version(), Some(BufferVersion::new(1)));
        assert!(!changes.patch().is_empty());
    }
}
