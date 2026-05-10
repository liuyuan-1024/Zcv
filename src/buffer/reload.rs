//! Reload 与保存边界：替换外部文本基线、清理编辑历史，并输出待保存文本。
//!
//! 本文件不做文件 I/O、不监听外部变化，也不决定冲突交互；宿主只把文本或快照交给 engine。

use crate::{
    BufferVersion, EngineError, EngineResult, LineEndingConfig, SelectionSet, Snapshot,
    TransactionError, storage::RopeyStorage,
};

use super::Buffer;

impl Buffer {
    /// 用外部文本重新加载 Buffer。
    ///
    /// reload 表示外部文本源成为新的干净基线：文本存储重建、版本递增、history 清空、
    /// composition 取消、selection 回到文档开头，dirty 状态恢复为 clean。
    pub fn reload_from_text(&mut self, text: String) -> EngineResult<()> {
        self.storage = RopeyStorage::new(text);
        self.bump_version()?;
        self.history.clear();
        self.selection = SelectionSet::default();
        self.composition = None;
        self.loaded_text_info = None;
        self.mark_clean_internal();
        self.mark_synced_external();
        self.apply_large_file_auto_read_only();
        Ok(())
    }

    /// 用已有 Snapshot 重新加载 Buffer。
    ///
    /// 只读取 Snapshot 的文本内容；Buffer 身份、配置、只读状态和文件绑定保持不变。
    pub fn reload_from_snapshot(&mut self, snapshot: &Snapshot) -> EngineResult<()> {
        self.reload_from_text(snapshot.text().into_owned())
    }

    /// 输出待保存文本，并在输出前检查调用方持有的版本是否仍然新鲜。
    ///
    /// 这里不修改 Buffer 状态；宿主完成真实写盘后再调用 `mark_saved()` /
    /// `mark_synced_external()`。
    pub fn to_save_text(&self, expected_version: BufferVersion) -> EngineResult<String> {
        if expected_version != self.version {
            return Err(TransactionError::VersionMismatch {
                expected: self.version,
                actual: expected_version,
            }
            .into());
        }

        Ok(normalize_line_endings(
            self.text().as_ref(),
            self.config.line_ending,
        ))
    }

    pub(in crate::buffer) fn bump_version(&mut self) -> EngineResult<()> {
        self.version = self.version.next().ok_or(EngineError::VersionOverflow)?;
        Ok(())
    }
}

fn normalize_line_endings(text: &str, config: LineEndingConfig) -> String {
    match config {
        LineEndingConfig::Preserve => text.to_string(),
        LineEndingConfig::Lf => normalize_line_endings_to(text, "\n"),
        LineEndingConfig::Crlf => normalize_line_endings_to(text, "\r\n"),
        LineEndingConfig::Native => normalize_line_endings_to(text, native_line_ending()),
    }
}

fn normalize_line_endings_to(text: &str, target: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                output.push_str(target);
                index += 2;
            }
            b'\r' | b'\n' => {
                output.push_str(target);
                index += 1;
            }
            _ => {
                let ch = text[index..]
                    .chars()
                    .next()
                    .expect("index must be at a valid char boundary");
                output.push(ch);
                index += ch.len_utf8();
            }
        }
    }

    output
}

fn native_line_ending() -> &'static str {
    if cfg!(windows) { "\r\n" } else { "\n" }
}
