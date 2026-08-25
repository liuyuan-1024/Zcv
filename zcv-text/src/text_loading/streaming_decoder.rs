//! 流式 UTF-8 解码：把 `io::Read` 增量喂给 `ropey::RopeBuilder`，并应用 BOM 与非法 UTF-8 策略。

use std::io;

use ropey::{Rope, RopeBuilder};

use crate::{BomPolicy, BufferConfig, InvalidUtf8Policy, StorageError};

/// 单次 `read` 系统调用最多吃多少字节。
///
/// 选 64 KiB 是常见 page * 16 倍率，足够吃 ropey 的单 chunk 上限（一般 4 KiB），又不至于让最终残留（incomplete UTF-8 至多 3 字节）所占比例过低。
const READ_BUFFER_SIZE: usize = 64 * 1024;

/// UTF-8 BOM 字节序列。
const UTF8_BOM: &[u8; 3] = b"\xEF\xBB\xBF";

/// 流式解码 `reader` 为 UTF-8 文本，按 `config` 应用 BOM / 非法 UTF-8 策略。
///
/// **错误来源**：
/// - `io::Error` 由调用方 wrap（见 [`crate::BufferLoadError`]）；本函数仅在 `read` 返回 `Err` 时把它直接上抛。
/// - 非法 UTF-8（策略为 `Reject`）转 [`StorageError::InvalidUtf8`]。
///
/// 不完整 UTF-8 codepoint（最多 3 字节）保留在读缓冲首端，下一轮拼接。
pub(crate) fn decode_stream<R: io::Read>(
    mut reader: R,
    config: &BufferConfig,
) -> Result<Rope, StreamDecodeError> {
    let bom_policy = config.encoding.bom;
    let invalid_policy = config.encoding.invalid_utf8;

    let mut builder = RopeBuilder::new();
    let mut buffer = vec![0u8; READ_BUFFER_SIZE];
    let mut fill_idx = 0usize;

    let mut consumed_bytes = 0usize;
    let mut pending_bom_check = true;

    loop {
        let read_count = reader
            .read(&mut buffer[fill_idx..])
            .map_err(StreamDecodeError::Io)?;
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
                // 总长 <3 字节，不可能有 BOM。
                pending_bom_check = false;
            } else {
                // 继续读，下一轮再判。
                continue;
            }
        }

        // 找当前缓冲中最长的合法 UTF-8 前缀。
        let valid = classify_utf8(&buffer[..fill_idx]);

        // 把合法前缀喂给 rope。
        if valid.valid_count > 0 {
            // SAFETY: 上一行已通过 `std::str::from_utf8` / `classify_utf8` 校验过这段字节是合法 UTF-8。
            let s = unsafe { std::str::from_utf8_unchecked(&buffer[..valid.valid_count]) };
            builder.append(s);
            consumed_bytes += valid.valid_count;
        }

        match valid.tail_kind {
            TailKind::AllValid | TailKind::IncompleteCodepoint => {
                // 残留搬到缓冲前端等下一轮拼接；IncompleteCodepoint 至多 3 字节。
                let remaining = fill_idx - valid.valid_count;
                if remaining > 0 {
                    buffer.copy_within(valid.valid_count..fill_idx, 0);
                }
                fill_idx = remaining;
            }
            TailKind::InvalidBytes(bad_len) => match invalid_policy {
                InvalidUtf8Policy::Reject => {
                    return Err(StreamDecodeError::Text(
                        StorageError::InvalidUtf8 {
                            valid_up_to: consumed_bytes,
                            error_len: Some(bad_len),
                        }
                        .into(),
                    ));
                }
                InvalidUtf8Policy::Replace => {
                    const REPLACEMENT: &str = "\u{FFFD}";
                    builder.append(REPLACEMENT);
                    consumed_bytes += REPLACEMENT.len();

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
                        return Err(StreamDecodeError::Text(
                            StorageError::InvalidUtf8 {
                                valid_up_to: consumed_bytes,
                                error_len: None,
                            }
                            .into(),
                        ));
                    }
                    InvalidUtf8Policy::Replace => {
                        const REPLACEMENT: &str = "\u{FFFD}";
                        builder.append(REPLACEMENT);
                    }
                }
            }
            break;
        }

        // 缓冲被一个迭代填满且无法消费——按 ropey 同样的逻辑视为非法（codepoint 不会到 64 KiB）。
        // Replace 策略下能走到这里说明实现有 bug，仍按 Reject 上抛。
        if fill_idx == READ_BUFFER_SIZE {
            return Err(StreamDecodeError::Text(
                StorageError::InvalidUtf8 {
                    valid_up_to: consumed_bytes,
                    error_len: None,
                }
                .into(),
            ));
        }
    }

    Ok(builder.finish())
}

/// `decode_stream` 失败原因。
///
/// 单独抽出这层以允许调用方（[`crate::Buffer::from_reader`]）把 IO 与解码错误上升为统一的 [`crate::BufferLoadError`]。
#[derive(Debug)]
pub(crate) enum StreamDecodeError {
    Io(io::Error),
    Text(crate::TextError),
}

struct ValidPrefix {
    valid_count: usize,
    tail_kind: TailKind,
}

enum TailKind {
    AllValid,
    /// 尾部是合法 UTF-8 多字节序列的开头（≤3 字节），等待下一 chunk 续命。
    IncompleteCodepoint,
    /// 尾部存在 `n` 字节不可恢复非法序列（与 `Utf8Error::error_len` 对齐）。
    InvalidBytes(usize),
}

fn classify_utf8(bytes: &[u8]) -> ValidPrefix {
    match std::str::from_utf8(bytes) {
        Ok(_) => ValidPrefix {
            valid_count: bytes.len(),
            tail_kind: TailKind::AllValid,
        },
        Err(e) => {
            let valid_count = e.valid_up_to();
            let tail_kind = match e.error_len() {
                None => TailKind::IncompleteCodepoint,
                Some(n) => TailKind::InvalidBytes(n),
            };
            ValidPrefix {
                valid_count,
                tail_kind,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BufferConfig, EncodingConfig, TextError};

    fn decode(bytes: &[u8]) -> Rope {
        decode_stream(bytes, &BufferConfig::default()).expect("decode")
    }

    #[test]
    fn empty_input_yields_empty_rope() {
        assert_eq!(decode(b"").to_string(), "");
    }

    #[test]
    fn bom_is_stripped_by_default() {
        assert_eq!(decode(b"\xEF\xBB\xBFhello").to_string(), "hello");
    }

    #[test]
    fn bom_preserved_when_policy_says_so() {
        let cfg = BufferConfig {
            encoding: EncodingConfig::new(BomPolicy::Preserve, InvalidUtf8Policy::Reject),
            ..BufferConfig::default()
        };
        let r = decode_stream(&b"\xEF\xBB\xBFhi"[..], &cfg).expect("decode");
        assert_eq!(r.to_string(), "\u{FEFF}hi");
    }

    #[test]
    fn invalid_utf8_rejected_by_default() {
        let err = decode_stream(&b"abc\xFFdef"[..], &BufferConfig::default()).unwrap_err();
        assert!(matches!(
            err,
            StreamDecodeError::Text(TextError::Storage(StorageError::InvalidUtf8 { .. }))
        ));
    }

    #[test]
    fn invalid_utf8_replaced_when_policy_says_so() {
        let cfg = BufferConfig {
            encoding: EncodingConfig::new(BomPolicy::Strip, InvalidUtf8Policy::Replace),
            ..BufferConfig::default()
        };
        let r = decode_stream(&b"a\xFFb"[..], &cfg).expect("decode");
        assert_eq!(r.to_string(), "a\u{FFFD}b");
    }

    /// 跨块边界的 UTF-8 多字节字符：把 "héllo" 拆成"h" + "é的1字节" + "é的2字节
    /// + llo" 三段。decoder 必须正确拼回 é。
    #[test]
    fn multibyte_codepoint_split_across_reads() {
        let bytes = "héllo\n".as_bytes(); // 7 bytes; é = 0xC3 0xA9
        let chunks: Vec<&[u8]> = vec![&bytes[..1], &bytes[1..2], &bytes[2..]];
        let r =
            decode_stream(ChunkedReader::new(chunks), &BufferConfig::default()).expect("decode");
        assert_eq!(r.to_string(), "héllo\n");
    }

    /// 64 KB 缓冲下，> 64 KB 的输入要分多次 read，结果与单次 read 等价。
    #[test]
    fn large_input_matches_single_chunk_equivalent() {
        let mut s = String::new();
        for i in 0..10_000 {
            s.push_str(&format!("line {i} with some text\n"));
        }
        let single = decode(s.as_bytes());
        let chunked = decode_stream(
            ChunkedReader::new(s.as_bytes().chunks(1024).collect()),
            &BufferConfig::default(),
        )
        .expect("decode");
        assert_eq!(single.to_string(), chunked.to_string());
    }

    /// 单测专用 reader：按预设字节段顺序一次返回一段。
    struct ChunkedReader<'a> {
        chunks: Vec<&'a [u8]>,
        idx: usize,
    }
    impl<'a> ChunkedReader<'a> {
        fn new(chunks: Vec<&'a [u8]>) -> Self {
            Self { chunks, idx: 0 }
        }
    }
    impl<'a> io::Read for ChunkedReader<'a> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.idx >= self.chunks.len() {
                return Ok(0);
            }
            let src = self.chunks[self.idx];
            let n = src.len().min(buf.len());
            buf[..n].copy_from_slice(&src[..n]);
            if n < src.len() {
                self.chunks[self.idx] = &src[n..];
            } else {
                self.idx += 1;
            }
            Ok(n)
        }
    }
}
