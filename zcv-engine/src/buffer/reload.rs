//! Reload 与保存边界：替换外部文本基线、清理编辑历史，并流式输出待保存文本。
//!
//! 本文件不做文件 I/O、不监听外部变化，也不决定冲突交互；宿主只把文本或快照交给 engine。

use std::io::{self, Write};

use super::Buffer;
use crate::{
    BufferSaveError, BufferVersion, ByteOffset, EngineError, EngineResult, LineEndingConfig,
    TextRange, TransactionError,
    diff::diff_patch,
    storage::{RopeyStorage, TextRead},
};

impl Buffer {
    /// 用外部文本重新加载 Buffer。
    ///
    /// reload 表示外部文本源成为新的干净基线：文本存储重建、版本递增、history 清空，dirty 状态恢复为 clean。
    /// 对外发布旧文本 -> 新文本的 diff patch，使选区 / 折叠端点跟随外部变更后的具体位置。
    pub fn reload_from_text(&mut self, text: String) -> EngineResult<()> {
        let old_version = self.version;
        // 替换存储前取出旧文本，生成真实的坐标映射 patch。
        let old_text = self.storage.slice_to_string(
            TextRange::new(ByteOffset::ZERO, self.storage.len_bytes())
                .expect("全文范围必须满足 start <= end"),
        )?;
        let patch = diff_patch(&old_text, &text);
        let new_storage = RopeyStorage::new(text);
        let new_version = self.version.next().ok_or(EngineError::VersionOverflow)?;
        self.storage = new_storage;
        self.version = new_version;
        self.history.clear();
        self.loaded_text_info = None;
        self.mark_clean_internal();
        self.mark_synced_external();
        self.apply_large_file_auto_read_only();
        self.text_changes
            .publish(old_version, self.version, patch, false);
        Ok(())
    }

    /// 流式输出待保存文本，并在输出前检查调用方持有的版本是否仍然新鲜。
    ///
    /// 这里不修改 Buffer 状态；宿主完成真实写盘后再调用 `mark_saved()` / `mark_synced_external()`。
    pub fn write_to<W: Write>(
        &self,
        expected_version: BufferVersion,
        mut writer: W,
    ) -> Result<(), BufferSaveError> {
        if expected_version != self.version {
            return Err(EngineError::from(TransactionError::VersionMismatch {
                expected: self.version,
                actual: expected_version,
            })
            .into());
        }

        let range = TextRange::new(ByteOffset::ZERO, self.storage.len_bytes())
            .map_err(EngineError::from)?;
        let chunks = self.storage.chunks(range)?;
        match self.config.line_ending {
            LineEndingConfig::Preserve => write_preserved_line_endings(&mut writer, chunks)?,
            LineEndingConfig::Lf => write_normalized_line_endings(&mut writer, chunks, "\n")?,
            LineEndingConfig::Crlf => write_normalized_line_endings(&mut writer, chunks, "\r\n")?,
            LineEndingConfig::Native => {
                write_normalized_line_endings(&mut writer, chunks, native_line_ending())?
            }
        }
        writer.flush()?;
        Ok(())
    }
}

fn write_preserved_line_endings<'a, W, I>(writer: &mut W, chunks: I) -> io::Result<()>
where
    W: Write,
    I: IntoIterator<Item = &'a str>,
{
    for chunk in chunks {
        writer.write_all(chunk.as_bytes())?;
    }
    Ok(())
}

fn write_normalized_line_endings<'a, W, I>(
    writer: &mut W,
    chunks: I,
    target: &str,
) -> io::Result<()>
where
    W: Write,
    I: IntoIterator<Item = &'a str>,
{
    let target = target.as_bytes();
    let mut pending_cr = false;

    for chunk in chunks {
        let bytes = chunk.as_bytes();
        let mut index = 0usize;
        let mut segment_start = 0usize;

        if pending_cr {
            writer.write_all(target)?;
            pending_cr = false;
            if bytes.first() == Some(&b'\n') {
                index = 1;
                segment_start = 1;
            }
        }

        while index < bytes.len() {
            match bytes[index] {
                b'\r' => {
                    writer.write_all(&bytes[segment_start..index])?;
                    if bytes.get(index + 1) == Some(&b'\n') {
                        writer.write_all(target)?;
                        index += 2;
                        segment_start = index;
                    } else if index + 1 == bytes.len() {
                        pending_cr = true;
                        index += 1;
                        segment_start = index;
                    } else {
                        writer.write_all(target)?;
                        index += 1;
                        segment_start = index;
                    }
                }
                b'\n' => {
                    writer.write_all(&bytes[segment_start..index])?;
                    writer.write_all(target)?;
                    index += 1;
                    segment_start = index;
                }
                _ => {
                    index += 1;
                }
            }
        }

        writer.write_all(&bytes[segment_start..])?;
    }

    if pending_cr {
        writer.write_all(target)?;
    }
    Ok(())
}

fn native_line_ending() -> &'static str {
    if cfg!(windows) { "\r\n" } else { "\n" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_line_endings_should_handle_crlf_split_across_chunks() {
        let mut out = Vec::new();

        write_normalized_line_endings(&mut out, ["a\r", "\nb\r", "c\n"], "\n").unwrap();

        assert_eq!(String::from_utf8(out).unwrap(), "a\nb\nc\n");
    }
}
