//! 基础编辑入口：为 Buffer 提供 insert/delete/replace 这组三个单范围便利 API。
//!
//! 本文件只把简单编辑统一转换成 Transaction 并进入历史链路；批量 selection 编辑和事务提交细节由相邻子系统承担。

use crate::{
    ByteOffset, EngineResult, TextRange,
    transaction::{Edit, Transaction},
};

use crate::buffer::Buffer;

impl Buffer {
    pub fn insert(&mut self, offset: ByteOffset, text: &str) -> EngineResult<()> {
        let range = TextRange::new(offset, offset)?;
        self.replace(range, text)
    }

    pub fn delete(&mut self, range: TextRange) -> EngineResult<()> {
        self.replace(range, "")
    }

    /// 替换指定字符范围的文本，支持插入和删除。
    ///
    /// 该便利 API 内部走 Transaction，会进入 Undo 历史。
    pub fn replace(&mut self, range: TextRange, replacement: &str) -> EngineResult<()> {
        self.ensure_writable()?;
        self.cancel_composition_before_text_edit()?;
        self.validate_range(range)?;
        self.validate_edit_boundary(range.start())?;
        self.validate_edit_boundary(range.end())?;

        // no-op 不递增版本，也不污染 dirty / history。
        if self.slice_text(range)?.as_ref() == replacement {
            return Ok(());
        }

        let tx = Transaction::from_edits(
            self.version,
            vec![Edit::replace(range, replacement.to_string())],
        )?;

        self.apply_transaction(tx)?;
        Ok(())
    }
}
