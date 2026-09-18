//! Reload 与保存边界：替换外部文本基线、清理编辑历史，并流式输出待保存文本。
//!
//! 本文件不做文件 I/O、不监听外部变化，也不决定冲突交互；宿主只把文本或快照交给 engine。

use std::io::{self, Write};

use super::{Buffer, platform};
use crate::{
    BufferSaveError, BufferVersion, ByteOffset, TextError, TextRange, TextResult,
    config::LineEndingConfig,
    diff::diff_patch,
    errors::TransactionError,
    storage::{RopeyStorage, TextRead},
    transaction::{Edit, EditList, TransactionSource},
};

impl Buffer {
    /// 用外部文本重新加载 Buffer。
    ///
    /// reload 表示外部文本源成为新的干净基线。
    /// 文本发生变化时重建存储、递增版本并清空 history；
    /// 文本相同时只推进保存点，保留现有版本和 history。
    /// 两种情况都会把 dirty 状态恢复为 clean。
    /// 对外发布旧文本 -> 新文本的 diff patch，使选区 / 折叠端点跟随外部变更后的具体位置。
    pub fn reload_from_text(&mut self, text: String) -> TextResult<()> {
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
        let (next_transaction_id, event) =
            self.prepare_delta_event(old_version, edits, TransactionSource::Programmatic, true)?;
        self.commit_prepared_text_change(new_storage, next_transaction_id, &event);
        self.history.clear();
        self.session = None;
        self.mark_clean_internal();
        self.apply_large_file_auto_read_only();
        Ok(())
    }

    /// 流式输出待保存文本，并在输出前检查调用方持有的版本是否仍然新鲜。
    ///
    /// 这里不修改 Buffer 状态；宿主完成真实写盘后再调用 `mark_saved()`。
    pub fn write_to<W: Write>(
        &self,
        expected_version: BufferVersion,
        mut writer: W,
    ) -> Result<(), BufferSaveError> {
        if expected_version != self.version {
            return Err(TextError::from(TransactionError::VersionMismatch {
                expected: self.version,
                actual: expected_version,
            })
            .into());
        }

        let range =
            TextRange::new(ByteOffset::ZERO, self.storage.len_bytes()).map_err(TextError::from)?;
        let chunks = self.storage.chunks(range)?;
        match self.config.line_ending {
            LineEndingConfig::Preserve => write_preserved_line_endings(&mut writer, chunks)?,
            LineEndingConfig::Lf => write_normalized_line_endings(&mut writer, chunks, "\n")?,
            LineEndingConfig::Crlf => write_normalized_line_endings(&mut writer, chunks, "\r\n")?,
            LineEndingConfig::Native => {
                write_normalized_line_endings(&mut writer, chunks, platform::native_line_ending())?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_marks_text_changes_as_a_reset() {
        let mut buffer = Buffer::from_text("before\n".to_owned(), Default::default()).unwrap();
        let subscription = buffer.subscribe();

        buffer.reload_from_text("after\n".to_owned()).unwrap();

        let changes = subscription.consume();
        assert!(changes.requires_reset());
        assert!(changes.transaction_id().is_some());
        assert_eq!(changes.old_version(), Some(BufferVersion::INITIAL));
        assert_eq!(changes.new_version(), Some(BufferVersion::new(1)));
        assert!(!changes.patch().is_empty());
    }

    #[test]
    fn normalize_line_endings_should_handle_crlf_split_across_chunks() {
        let mut out = Vec::new();

        write_normalized_line_endings(&mut out, ["a\r", "\nb\r", "c\n"], "\n").unwrap();

        assert_eq!(String::from_utf8(out).unwrap(), "a\nb\nc\n");
    }
}
