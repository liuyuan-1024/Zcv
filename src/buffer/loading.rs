//! 文件文本加载入口：把外部 UTF-8 bytes 解码为 Buffer 文本并记录加载元信息。
//!
//! 本文件只负责 M7C 的文本进入边界；reload、保存输出、编码转换和文件监听属于后续阶段或宿主层。

use crate::{
    BomPolicy, BufferConfig, BufferKind, EngineResult, InvalidUtf8Policy, LineEndingStyle,
    LoadedTextInfo, StorageError, TextEncoding,
};

use super::Buffer;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

impl Buffer {
    /// 从外部 UTF-8 bytes 创建 Buffer，并记录 BOM、非法 UTF-8、换行风格和末尾换行状态。
    ///
    /// 这里不做文件 I/O；宿主负责读取 bytes，engine 只负责把 bytes 变成可编辑文本。
    pub fn from_loaded_text(
        kind: BufferKind,
        bytes: impl AsRef<[u8]>,
        config: BufferConfig,
    ) -> EngineResult<Self> {
        let (text, info) = decode_loaded_text(bytes.as_ref(), config.encoding)?;
        let mut buffer = Self::from_kind_text(kind, text, config)?;
        buffer.loaded_text_info = Some(info);
        buffer.mark_synced_external();
        Ok(buffer)
    }
}

fn decode_loaded_text(
    bytes: &[u8],
    encoding: crate::EncodingConfig,
) -> EngineResult<(String, LoadedTextInfo)> {
    let had_bom = bytes.starts_with(UTF8_BOM);
    let content = if had_bom && encoding.bom == BomPolicy::Strip {
        &bytes[UTF8_BOM.len()..]
    } else {
        bytes
    };

    let (text, had_invalid_utf8) = match std::str::from_utf8(content) {
        Ok(text) => (text.to_string(), false),
        Err(error) => match encoding.invalid_utf8 {
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

    let info = LoadedTextInfo::new(
        TextEncoding::Utf8,
        encoding.bom,
        encoding.invalid_utf8,
        had_bom,
        had_invalid_utf8,
        detect_line_ending_style(&text),
        has_final_newline(&text),
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
