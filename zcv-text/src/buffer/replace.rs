//! 外部文本更新：按差异把新文本应用到 Buffer。
//!
//! 文件 IO、编码恢复、保存点与保存策略属于上层文件边界；
//! 本模块只接收已解码的 `String`，不表达保存语义。

use super::Buffer;
use crate::{ByteOffset, TextRange, TextResult, diff::diff_edits, storage::TextRead};

impl Buffer {
    /// 用外部文本更新 Buffer。
    ///
    /// 文本变化时，和 Zed 的 reload 一样先计算旧文本到新文本的差异，再经 T-9 派生快照路径安装；
    /// 锚点、语法树和显示投影因此共享同一条增量坐标链。
    /// 保存点由上层文件边界在写入磁盘后显式推进，本方法不标记保存状态。
    pub fn replace_text(&mut self, text: String) -> TextResult<()> {
        let old_text = self.full_text()?;
        if old_text == text {
            return Ok(());
        }

        let edited = self.snapshot_with_edits(diff_edits(&old_text, &text))?;
        self.fast_forward(edited)?;
        self.apply_large_file_auto_read_only();
        Ok(())
    }

    fn full_text(&self) -> TextResult<String> {
        self.storage.slice_to_string(
            TextRange::new(ByteOffset::ZERO, self.storage.len_bytes())
                .expect("全文范围必须满足 start <= end"),
        )
    }
}

#[cfg(test)]
#[path = "test/replace_tests.rs"]
mod tests;
