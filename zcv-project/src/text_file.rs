//! 文件文本边界：UTF-8/BOM 解码、换行规范化与 Buffer 的加载 / 保存。
//!
//! `zcv-text` 只接收已解码的 `String` 并暴露检测到的换行风格；
//! 文件 IO、编码恢复与保存策略都由本模块拥有。

use std::io::{self, Write};

use zcv_text::{
    Buffer, BufferVersion, ByteOffset, TextError, TextRange, TextRead, TransactionError,
};

/// 单次 `read` 系统调用最多吃多少字节。
const READ_BUFFER_SIZE: usize = 64 * 1024;

/// UTF-8 BOM 字节序列。
const UTF8_BOM: &[u8; 3] = b"\xEF\xBB\xBF";

/// UTF-8 BOM 进入 Buffer 文本时的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BomPolicy {
    /// 识别并移除 UTF-8 BOM。
    #[default]
    Strip,
    /// 把 BOM 作为 U+FEFF 保留在 Buffer 文本中。
    Preserve,
}

/// 非法 UTF-8 字节的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum InvalidUtf8Policy {
    /// 遇到非法 UTF-8 直接返回错误。
    #[default]
    Reject,
    /// 使用 Unicode replacement character 恢复为可编辑文本。
    Replace,
}

/// 文件加载时的编码恢复策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodingConfig {
    pub bom: BomPolicy,
    pub invalid_utf8: InvalidUtf8Policy,
}

impl EncodingConfig {
    pub const fn new(bom: BomPolicy, invalid_utf8: InvalidUtf8Policy) -> Self {
        Self { bom, invalid_utf8 }
    }
}

impl Default for EncodingConfig {
    fn default() -> Self {
        Self {
            bom: BomPolicy::Strip,
            invalid_utf8: InvalidUtf8Policy::Reject,
        }
    }
}

/// 保存文本时采用的换行策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEndingConfig {
    /// 强制写成 LF。
    Lf,
    /// 强制写成 CRLF。
    Crlf,
    /// 保留 Buffer 中的原始换行。
    #[default]
    Preserve,
    /// 使用当前平台的原生换行。
    Native,
}

/// 文件加载失败的统一错误类型。
#[derive(Debug)]
pub enum BufferLoadError {
    Io(io::Error),
    InvalidUtf8 {
        valid_up_to: usize,
        error_len: Option<usize>,
    },
    Text(TextError),
}

impl From<io::Error> for BufferLoadError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<TextError> for BufferLoadError {
    fn from(value: TextError) -> Self {
        Self::Text(value)
    }
}

impl std::fmt::Display for BufferLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "文件加载 IO 失败：{error}"),
            Self::InvalidUtf8 {
                valid_up_to,
                error_len,
            } => write!(
                f,
                "文件不是合法 UTF-8：valid_up_to {valid_up_to}，error_len {error_len:?}"
            ),
            Self::Text(error) => write!(f, "文件解码失败：{error}"),
        }
    }
}

impl std::error::Error for BufferLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Text(error) => Some(error),
            Self::InvalidUtf8 { .. } => None,
        }
    }
}

/// 文件保存失败的统一错误类型。
#[derive(Debug)]
pub enum BufferSaveError {
    Io(io::Error),
    Text(TextError),
}

impl From<io::Error> for BufferSaveError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<TextError> for BufferSaveError {
    fn from(value: TextError) -> Self {
        Self::Text(value)
    }
}

impl std::fmt::Display for BufferSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "文件保存 IO 失败：{error}"),
            Self::Text(error) => write!(f, "文件保存校验失败：{error}"),
        }
    }
}

impl std::error::Error for BufferSaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Text(error) => Some(error),
        }
    }
}

/// 流式解码 `reader` 为 UTF-8 文本，按 `config` 应用 BOM / 非法 UTF-8 策略。
///
/// 不完整 UTF-8 codepoint（最多 3 字节）保留在读缓冲首端，下一轮拼接。
pub fn decode_to_string<R: io::Read>(
    mut reader: R,
    config: &EncodingConfig,
) -> Result<String, BufferLoadError> {
    let bom_policy = config.bom;
    let invalid_policy = config.invalid_utf8;

    let mut output = String::new();
    let mut buffer = vec![0u8; READ_BUFFER_SIZE];
    let mut fill_idx = 0usize;

    let mut consumed_bytes = 0usize;
    let mut pending_bom_check = true;

    loop {
        let read_count = reader.read(&mut buffer[fill_idx..])?;
        let eof = read_count == 0;
        fill_idx += read_count;

        // BOM 检查（first-time only）——必须在 UTF-8 校验之前。
        // 因为 BOM 本身是合法 UTF-8（U+FEFF），不剥离会让它落进文本首字符。
        if pending_bom_check {
            if fill_idx >= UTF8_BOM.len() {
                if buffer.starts_with(UTF8_BOM) && bom_policy == BomPolicy::Strip {
                    buffer.copy_within(UTF8_BOM.len()..fill_idx, 0);
                    fill_idx -= UTF8_BOM.len();
                }
                pending_bom_check = false;
            } else if eof {
                pending_bom_check = false;
            } else {
                continue;
            }
        }

        let valid = classify_utf8(&buffer[..fill_idx]);
        if valid.valid_count > 0 {
            // SAFETY: 上一行已通过 `std::str::from_utf8` / `classify_utf8` 校验过这段字节是合法 UTF-8。
            let text = unsafe { std::str::from_utf8_unchecked(&buffer[..valid.valid_count]) };
            output.push_str(text);
            consumed_bytes += valid.valid_count;
        }

        match valid.tail_kind {
            TailKind::AllValid | TailKind::IncompleteCodepoint => {
                let remaining = fill_idx - valid.valid_count;
                if remaining > 0 {
                    buffer.copy_within(valid.valid_count..fill_idx, 0);
                }
                fill_idx = remaining;
            }
            TailKind::InvalidBytes(bad_len) => match invalid_policy {
                InvalidUtf8Policy::Reject => {
                    return Err(BufferLoadError::InvalidUtf8 {
                        valid_up_to: consumed_bytes,
                        error_len: Some(bad_len),
                    });
                }
                InvalidUtf8Policy::Replace => {
                    const REPLACEMENT: char = '\u{FFFD}';
                    output.push(REPLACEMENT);
                    consumed_bytes += REPLACEMENT.len_utf8();

                    let skip_to = valid.valid_count + bad_len;
                    let remaining = fill_idx - skip_to;
                    if remaining > 0 {
                        buffer.copy_within(skip_to..fill_idx, 0);
                    }
                    fill_idx = remaining;
                }
            },
        }

        if eof {
            if fill_idx > 0 {
                // 最后一段 incomplete codepoint 没有续命机会了。
                match invalid_policy {
                    InvalidUtf8Policy::Reject => {
                        return Err(BufferLoadError::InvalidUtf8 {
                            valid_up_to: consumed_bytes,
                            error_len: None,
                        });
                    }
                    InvalidUtf8Policy::Replace => output.push('\u{FFFD}'),
                }
            }
            break;
        }

        // 缓冲被一个迭代填满且无法消费——按 ropey 同样的逻辑视为非法（codepoint 不会到 64 KiB）。
        if fill_idx == READ_BUFFER_SIZE {
            return Err(BufferLoadError::InvalidUtf8 {
                valid_up_to: consumed_bytes,
                error_len: None,
            });
        }
    }

    Ok(output)
}

/// 把 Buffer 文本写入 `writer`，先校验调用方持有的版本仍然新鲜。
///
/// 不修改 Buffer 状态；宿主完成真实写盘后再调用 `Buffer::mark_saved()`。
pub fn write_buffer_to<W: Write>(
    buffer: &Buffer,
    expected_version: BufferVersion,
    writer: &mut W,
    line_ending: LineEndingConfig,
) -> Result<(), BufferSaveError> {
    if expected_version != buffer.version() {
        return Err(BufferSaveError::Text(TextError::Transaction(
            TransactionError::VersionMismatch {
                expected: buffer.version(),
                actual: expected_version,
            },
        )));
    }

    let snapshot = buffer.snapshot();
    let range = TextRange::new(ByteOffset::ZERO, snapshot.len_bytes()).map_err(TextError::from)?;
    let chunks = snapshot.chunks(range)?;
    match line_ending {
        LineEndingConfig::Preserve => write_preserved_line_endings(writer, chunks)?,
        LineEndingConfig::Lf => write_normalized_line_endings(writer, chunks, "\n")?,
        LineEndingConfig::Crlf => write_normalized_line_endings(writer, chunks, "\r\n")?,
        LineEndingConfig::Native => {
            write_normalized_line_endings(writer, chunks, native_line_ending())?
        }
    }
    writer.flush()?;
    Ok(())
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
    #[cfg(windows)]
    {
        "\r\n"
    }

    #[cfg(not(windows))]
    {
        "\n"
    }
}

struct ValidPrefix {
    valid_count: usize,
    tail_kind: TailKind,
}

enum TailKind {
    AllValid,
    /// 尾部是合法 UTF-8 多字节序列的开头（≤3 字节），等待下一 chunk 续命。
    IncompleteCodepoint,
    /// 尾部存在 `n` 字节不可恢复非法序列。
    InvalidBytes(usize),
}

#[cfg(test)]
#[path = "test/text_file_tests.rs"]
mod tests;

fn classify_utf8(bytes: &[u8]) -> ValidPrefix {
    match std::str::from_utf8(bytes) {
        Ok(_) => ValidPrefix {
            valid_count: bytes.len(),
            tail_kind: TailKind::AllValid,
        },
        Err(error) => {
            let valid_count = error.valid_up_to();
            let tail_kind = match error.error_len() {
                None => TailKind::IncompleteCodepoint,
                Some(length) => TailKind::InvalidBytes(length),
            };
            ValidPrefix {
                valid_count,
                tail_kind,
            }
        }
    }
}
