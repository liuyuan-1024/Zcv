//! 文件文本加载入口：把外部 UTF-8 bytes 解码为 Buffer 文本并记录加载元信息。
//!
//! 本文件只负责文本的进入边界；reload、保存输出、编码转换和文件监听属于宿主层。

use crate::{
    BomPolicy, BufferConfig, BufferOrigin, ByteOffset, EngineResult, InvalidUtf8Policy,
    LargeFilePolicy, LineEndingStyle, LoadedTextInfo, StorageError, TextEncoding,
};

use super::Buffer;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

impl Buffer {
    /// 从外部 UTF-8 bytes 创建 Buffer，并记录 BOM、非法 UTF-8、换行风格和末尾换行状态。
    ///
    /// 这里不做文件 I/O；宿主负责读取 bytes，engine 只负责把 bytes 变成可编辑文本。
    pub fn from_loaded_text(
        origin: BufferOrigin,
        bytes: impl AsRef<[u8]>,
        config: BufferConfig,
    ) -> EngineResult<Self> {
        let large_file_policy = config.large_file.clone();
        let (text, info) = decode_loaded_text(bytes.as_ref(), &config, &large_file_policy)?;
        let mut buffer = Self::with_origin(origin, text, config)?;
        buffer.loaded_text_info = Some(info);
        buffer.mark_synced_external();
        Ok(buffer)
    }
}

fn decode_loaded_text(
    bytes: &[u8],
    config: &BufferConfig,
    large_file_policy: &LargeFilePolicy,
) -> EngineResult<(String, LoadedTextInfo)> {
    let had_bom = bytes.starts_with(UTF8_BOM);
    let content = if had_bom && config.encoding.bom == BomPolicy::Strip {
        &bytes[UTF8_BOM.len()..]
    } else {
        bytes
    };

    let (text, had_invalid_utf8) = match std::str::from_utf8(content) {
        Ok(text) => (text.to_string(), false),
        Err(error) => match config.encoding.invalid_utf8 {
            InvalidUtf8Policy::Reject => {
                return Err(StorageError::InvalidUtf8 {
                    valid_up_to: error.valid_up_to(),
                    error_len: error.error_len(),
                }
                .into());
            }
            InvalidUtf8Policy::Replace => (String::from_utf8_lossy(content).into_owned(), true),
        },
    };

    let loaded_byte_size = text.len();
    let longest_line_chars = longest_line_chars_in(&text);
    let info = LoadedTextInfo::new(
        TextEncoding::Utf8,
        config.encoding.bom,
        config.encoding.invalid_utf8,
        had_bom,
        had_invalid_utf8,
        detect_line_ending_style(&text),
        has_final_newline(&text),
        ByteOffset::new(loaded_byte_size),
        large_file_policy.is_large_byte_size(loaded_byte_size),
        longest_line_chars,
        large_file_policy.is_long_line(longest_line_chars),
    );

    Ok((text, info))
}

fn detect_line_ending_style(text: &str) -> LineEndingStyle {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    let mut saw_lf = false;
    let mut saw_crlf = false;
    let mut saw_lone_cr = false;

    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                saw_crlf = true;
                index += 2;
            }
            b'\r' => {
                saw_lone_cr = true;
                index += 1;
            }
            b'\n' => {
                saw_lf = true;
                index += 1;
            }
            _ => index += 1,
        }
    }

    match (saw_lf, saw_crlf, saw_lone_cr) {
        (false, false, false) => LineEndingStyle::None,
        (true, false, false) => LineEndingStyle::Lf,
        (false, true, false) => LineEndingStyle::Crlf,
        _ => LineEndingStyle::Mixed,
    }
}

fn has_final_newline(text: &str) -> bool {
    text.ends_with('\n') || text.ends_with('\r')
}

/// 扫描 `text` 中每一行的字符数（不含行尾 LF / CRLF / 单 CR），返回最大值。
///
/// 空文本返回 `0`；纯换行行（如 `"\n"`）每行字符数为 0；最后一段不带换行的内容
/// 也算作一行参与比较。
pub(in crate::buffer) fn longest_line_chars_in(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut max_chars = 0usize;
    let mut line_start = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                let chars = text[line_start..index].chars().count();
                if chars > max_chars {
                    max_chars = chars;
                }
                index += 2;
                line_start = index;
            }
            b'\r' | b'\n' => {
                let chars = text[line_start..index].chars().count();
                if chars > max_chars {
                    max_chars = chars;
                }
                index += 1;
                line_start = index;
            }
            _ => index += 1,
        }
    }

    if line_start < bytes.len() {
        let chars = text[line_start..].chars().count();
        if chars > max_chars {
            max_chars = chars;
        }
    }

    max_chars
}
