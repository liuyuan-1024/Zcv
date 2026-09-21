//! 外部文本更新：按差异把新文本应用到 Buffer，并刷新保存点。
//!
//! 文件 IO、编码恢复与保存策略属于 `zcv-project` 的文件边界；
//! 本模块只接收已解码的 `String`。

use super::Buffer;
use crate::{
    ByteOffset, TextRange, TextResult, diff::diff_edits, storage::TextRead,
    transaction::TransactionMetadata,
};

impl Buffer {
    /// 用外部文本更新 Buffer，并把新版本标记为保存点。
    ///
    /// 文本变化时，和 Zed 的 reload 一样先计算旧文本到新文本的差异，再作为普通版本化编辑提交；
    /// 锚点、语法树和显示投影因此共享同一条增量坐标链。
    /// 文本相同时只推进保存点。
    pub fn replace_text(&mut self, text: String) -> TextResult<()> {
        let old_text = self.storage.slice_to_string(
            TextRange::new(ByteOffset::ZERO, self.storage.len_bytes())
                .expect("全文范围必须满足 start <= end"),
        )?;
        if old_text == text {
            self.mark_saved();
            return Ok(());
        }

        self.edit(diff_edits(&old_text, &text), TransactionMetadata::default())?;
        self.mark_saved();
        self.apply_large_file_auto_read_only();
        Ok(())
    }
}

#[cfg(test)]
#[path = "test/replace_tests.rs"]
mod tests;
