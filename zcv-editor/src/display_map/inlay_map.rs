//! 行内提示（inlay）显示层：buffer 之上、fold 之下的文本注入。
//!
//! InlayMap：inlay 文本以注入式投影进入行文本（不占行数、不替换文本），消费链（测量/换行/渲染）只感知投影文本；行内坐标双轨（原始偏移 ↔ 投影偏移）。
//! 注入配置版本独立于 buffer 版本，变化时 fold 层整体重建（下游测量/换行依赖文本内容）。

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;

use zcv_multi_buffer::MultiBufferSnapshot;
use zcv_text::{ByteOffset, Line};

use super::chunk::InlayInfo;
use super::line_stream::{LineStream, StreamLineSource};

/// 行内提示：锚定 buffer 字节位置（插入在其后）+ 内容文本。
///
/// 本层只负责显示投影，不绑定数据来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Inlay {
    pub(crate) position: ByteOffset,
    pub(crate) text: String,
}

/// 投影快照：底层流 + 注入配置。
#[derive(Debug, Clone)]
pub(crate) struct InlaySnapshot {
    stream: LineStream,
    /// 按 position 排序的行内提示。
    inlays: Vec<Inlay>,
    /// 每个投影行的注入段表。
    /// 它只保存注入的锚点与共享文本，不缓存或拼接投影后的整行；
    /// 消费游标据此直接穿行于源文本和 inlay 文本之间。
    line_inlays: Arc<BTreeMap<Line, Arc<[InlayInfo]>>>,
    /// 注入配置版本（与 buffer 版本独立；变化时消费链整体重建）。
    version: u64,
}

#[derive(Debug, Clone)]
pub(super) struct InlayMap {
    snapshot: InlaySnapshot,
}

impl InlayMap {
    pub(super) fn new(stream: LineStream) -> (Self, InlaySnapshot) {
        let snapshot = InlaySnapshot {
            stream,
            inlays: Vec::new(),
            line_inlays: Arc::new(BTreeMap::new()),
            version: 0,
        };
        (
            Self {
                snapshot: snapshot.clone(),
            },
            snapshot,
        )
    }

    /// 推进输入流与注入配置；注入配置变化时版本递增（fold 层据此整体重建）。
    /// 流变化（buffer 编辑）不递增注入版本，由消费链按 buffer 版本处理。
    pub(super) fn read(&mut self, stream: LineStream, inlays: Vec<Inlay>) -> InlaySnapshot {
        let inlay_changed = self.snapshot.inlays != inlays;
        let line_inlays = line_inlays(&stream, &inlays);
        self.snapshot = InlaySnapshot {
            stream,
            version: self.snapshot.version + inlay_changed as u64,
            inlays,
            line_inlays,
        };
        self.snapshot.clone()
    }
}

impl InlaySnapshot {
    pub(crate) fn stream(&self) -> &LineStream {
        &self.stream
    }

    pub(super) fn buffer_snapshot(&self) -> &MultiBufferSnapshot {
        self.stream.buffer_snapshot()
    }

    /// 注入配置版本（inlay 变化信号；stream 变化不递增，buffer 编辑由消费链自行处理）。
    pub(super) const fn version(&self) -> u64 {
        self.version
    }

    /// 统一行总数（inlay 不占行数）。
    pub(super) fn line_count(&self) -> usize {
        self.stream.line_count()
    }

    /// 流行号 → 来源（委托流）。
    pub(super) fn source(&self, line: Line) -> Option<StreamLineSource> {
        self.stream.source(line)
    }

    /// 行的原始字节范围（委托流）。
    pub(super) fn line_byte_range(&self, line: Line) -> Option<Range<ByteOffset>> {
        self.stream.line_byte_range(line)
    }

    /// 行内容的源字节范围，不含行尾换行。
    pub(crate) fn line_content_byte_range(&self, line: Line) -> Option<Range<ByteOffset>> {
        let source = self.source(line)?;
        let range = self.stream.line_byte_range(Line::new(source.line()))?;
        let mut end = range.end;
        while end > range.start {
            let last = ByteOffset::new(end.get() - 1);
            let is_line_break = self
                .buffer_snapshot()
                .text_chunks(last..end)
                .next()
                .and_then(|chunk| chunk.text.as_bytes().first().copied())
                .is_some_and(|byte| byte == b'\n' || byte == b'\r');
            if !is_line_break {
                break;
            }
            end = last;
        }
        Some(range.start..end)
    }

    /// 仅供坐标计算和换行重建临时读取投影行文本。
    ///
    /// 连续行游标只在当前行的回调生命周期内借用这个临时值；
    /// 它不会进入 DisplaySnapshot，也不会成为跨帧的投影文本缓存。
    pub(super) fn line_text(&self, line: Line) -> Option<Cow<'_, str>> {
        let text = self.stream.line_text(line)?;
        let inlays = self.line_inlays(line);
        if inlays.is_empty() {
            return Some(text);
        }
        let mut output = String::with_capacity(
            text.len() + inlays.iter().map(|inlay| inlay.text.len()).sum::<usize>(),
        );
        let mut cursor = 0;
        for inlay in inlays {
            output.push_str(&text[cursor..inlay.anchor]);
            output.push_str(&inlay.text);
            cursor = inlay.anchor;
        }
        output.push_str(&text[cursor..]);
        Some(Cow::Owned(output))
    }

    /// 行的注入段信息（供渲染合成：anchor 为行内原始偏移，projected 为投影偏移）。
    pub(crate) fn line_inlays(&self, line: Line) -> &[InlayInfo] {
        self.line_inlays
            .get(&line)
            .map_or(&[], |inlays| inlays.as_ref())
    }

    /// 投影行的字节长度，不拼接源文本与 inlay 文本。
    ///
    /// WrapMap 的断行点位于这一坐标域；
    /// 渲染游标只需要这个长度来裁剪片段，不应为了求长度物化整行。
    pub(crate) fn projected_line_len(&self, line: Line) -> Option<usize> {
        let range = self.line_byte_range(line)?;
        Some(
            range.end.get() - range.start.get()
                + self
                    .line_inlays(line)
                    .iter()
                    .map(|inlay| inlay.text.len())
                    .sum::<usize>(),
        )
    }

    /// 投影行内容（不含换行）的字节长度和字符数。
    ///
    /// 折叠层只需要这两个几何量，不应为了统计它们拼接 anchor 行文本。
    pub(crate) fn projected_line_content_metrics(&self, line: Line) -> Option<(usize, usize)> {
        let content = self.line_content_byte_range(line)?;
        let content_len = content.end.get() - content.start.get();
        let mut byte = content.start.get();
        let mut chars = 0;
        while byte < content.end.get() {
            let (chunk, chunk_start) = self
                .buffer_snapshot()
                .chunk_at_byte(ByteOffset::new(byte))
                .ok()?;
            let start = byte - chunk_start.get();
            let end = (content.end.get() - chunk_start.get()).min(chunk.len());
            chars += chunk[start..end].chars().count();
            byte = chunk_start.get() + end;
        }
        let inlays = self.line_inlays(line);
        Some((
            content_len + inlays.iter().map(|inlay| inlay.text.len()).sum::<usize>(),
            chars
                + inlays
                    .iter()
                    .map(|inlay| inlay.text.chars().count())
                    .sum::<usize>(),
        ))
    }

    /// 行内原始字节偏移 → 投影偏移。
    ///
    /// 面向"字符起点"语义（光标/命中测试的列换算）：
    /// 锚定偏移处的字符在注入文本之后，计入此前注入长度；
    /// 锚定偏移本身的字节边界则落在注入前（渲染合成用严格小于，见 chunk.rs）。
    pub(super) fn to_projected_offset(&self, line: Line, byte: usize) -> usize {
        byte + self
            .line_inlays(line)
            .iter()
            .take_while(|inlay| inlay.anchor <= byte)
            .map(|inlay| inlay.text.len())
            .sum::<usize>()
    }

    /// 投影偏移 → 行内原始字节偏移；落在注入段内时吸附到锚定之后（不可逆，Left bias）。
    pub(super) fn to_original_offset(&self, line: Line, projected: usize) -> usize {
        let inlays = self.line_inlays(line);
        for inlay in inlays {
            if projected >= inlay.projected && projected < inlay.projected + inlay.text.len() {
                return inlay.anchor;
            }
        }
        projected
            - inlays
                .iter()
                .take_while(|inlay| inlay.projected + inlay.text.len() <= projected)
                .map(|inlay| inlay.text.len())
                .sum::<usize>()
    }
}

fn line_inlays(stream: &LineStream, inlays: &[Inlay]) -> Arc<BTreeMap<Line, Arc<[InlayInfo]>>> {
    if inlays.is_empty() {
        return Arc::new(BTreeMap::new());
    }
    let mut line_infos = BTreeMap::new();
    let lines = inlays
        .iter()
        .filter_map(|inlay| {
            stream
                .buffer_snapshot()
                .byte_to_position(inlay.position)
                .ok()
                .map(|position| position.line())
        })
        .collect::<BTreeSet<_>>();
    for line in lines {
        let Some(source) = stream.source(line) else {
            continue;
        };
        let Some(range) = stream.line_byte_range(Line::new(source.line())) else {
            continue;
        };
        let line_inlays = inlays
            .iter()
            .filter(|inlay| inlay.position >= range.start && inlay.position < range.end)
            .collect::<Vec<_>>();
        if line_inlays.is_empty() {
            continue;
        }
        let mut prefix = 0usize;
        let mut infos = Vec::with_capacity(line_inlays.len());
        for inlay in line_inlays {
            let anchor = inlay.position.get() - range.start.get();
            infos.push(InlayInfo {
                anchor,
                projected: anchor + prefix,
                text: Arc::from(inlay.text.as_str()),
            });
            prefix += inlay.text.len();
        }
        line_infos.insert(line, Arc::from(infos));
    }
    Arc::new(line_infos)
}

#[cfg(test)]
#[path = "test/inlay_map.rs"]
mod test;
