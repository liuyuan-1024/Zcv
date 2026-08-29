//! 行内提示（inlay）显示层：buffer 之上、fold 之下的文本注入。
//!
//! InlayMap：inlay 文本以注入式投影进入行文本（不占行数、不替换文本），消费链（测量/换行/渲染）只感知投影文本；行内坐标双轨（原始偏移 ↔ 投影偏移）。
//! 注入配置版本独立于 buffer 版本，变化时 fold 层整体重建（下游测量/换行依赖文本内容）。

use std::borrow::Cow;
use std::ops::Range;

use zcv_text::{ByteOffset, Line, Snapshot};

use super::chunk::InlayInfo;
use super::line_stream::{LineStream, StreamLineSource, StreamLineText};

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
        self.snapshot = InlaySnapshot {
            stream,
            version: self.snapshot.version + inlay_changed as u64,
            inlays,
        };
        self.snapshot.clone()
    }
}

impl InlaySnapshot {
    pub(crate) fn stream(&self) -> &LineStream {
        &self.stream
    }

    pub(super) fn buffer_snapshot(&self) -> &Snapshot {
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

    /// 行的原始字节范围（委托流；合成行为锚定行行首的伪坐标）。
    pub(super) fn line_byte_range(&self, line: Line) -> Option<Range<ByteOffset>> {
        self.stream.line_byte_range(line)
    }

    /// 投影行文本（含行内提示注入）：无注入时借用，有则自持。
    pub(super) fn line_text(&self, line: Line) -> Option<Cow<'_, str>> {
        // 行内提示只作用于 buffer 行；合成行是外部文本，无注入。
        let StreamLineSource::Buffer(buffer_line) = self.stream.source(line)? else {
            return Some(stream_text(self.stream.line_text(line)?));
        };
        let range = self.stream.line_byte_range(Line::new(buffer_line))?;
        let inlays = self.inlays_in(&range);
        if inlays.is_empty() {
            return Some(stream_text(self.stream.line_text(line)?));
        }
        let text = stream_text(self.stream.line_text(line)?);
        // 注入信息：投影偏移 = 锚定偏移（行内）+ 此前注入长度和。
        let mut infos = Vec::with_capacity(inlays.len());
        let mut prefix = 0usize;
        for inlay in inlays {
            let anchor = inlay.position.get() - range.start.get();
            infos.push(InlayInfo {
                anchor,
                projected: anchor + prefix,
                text: &inlay.text,
            });
            prefix += inlay.text.len();
        }
        // 注入坐标是含此前前缀的投影偏移：按锚定序从前往后注入（后注入的位置在注入后的文本中）。
        let mut projected = String::with_capacity(text.len() + prefix);
        projected.push_str(&text);
        for info in infos {
            projected.insert_str(info.projected, info.text);
        }
        Some(Cow::Owned(projected))
    }

    /// 行的注入段信息（供渲染合成：anchor 为行内原始偏移，projected 为投影偏移）。
    pub(crate) fn line_inlays(&self, line: Line) -> Vec<InlayInfo<'_>> {
        let Some(StreamLineSource::Buffer(buffer_line)) = self.stream.source(line) else {
            return Vec::new();
        };
        let Some(range) = self.stream.line_byte_range(Line::new(buffer_line)) else {
            return Vec::new();
        };
        let mut infos = Vec::new();
        let mut prefix = 0usize;
        for inlay in self.inlays_in(&range) {
            let anchor = inlay.position.get() - range.start.get();
            infos.push(InlayInfo {
                anchor,
                projected: anchor + prefix,
                text: &inlay.text,
            });
            prefix += inlay.text.len();
        }
        infos
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
        for inlay in &inlays {
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

    /// 字节范围（行首/行尾）内的注入段（保持 position 升序）。
    fn inlays_in(&self, range: &Range<ByteOffset>) -> Vec<&Inlay> {
        self.inlays
            .iter()
            .filter(|inlay| inlay.position >= range.start && inlay.position < range.end)
            .collect()
    }
}

/// 流文本 → 投影 Cow（借用透传；Buffer 变体携带流的借用）。
fn stream_text(text: StreamLineText<'_>) -> Cow<'_, str> {
    match text {
        StreamLineText::Buffer(cow) => cow,
        StreamLineText::Inserted(text) => Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zcv_text::BufferConfig;

    fn snapshot_with(text: &str, inlays: Vec<Inlay>) -> InlaySnapshot {
        let buffer = zcv_text::Buffer::scratch(text.to_owned(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let (mut map, _) = InlayMap::new(LineStream::new(buffer.snapshot()));
        map.read(LineStream::new(buffer.snapshot()), inlays)
    }

    fn inlay(position: usize, text: &str) -> Inlay {
        Inlay {
            position: ByteOffset::new(position),
            text: text.to_owned(),
        }
    }

    #[test]
    fn line_text_borrows_without_inlays() {
        let snapshot = snapshot_with("ab\ncd", Vec::new());
        let text = snapshot.line_text(Line::new(0)).expect("行 0 应可解析");
        assert_eq!(text, "ab\n");
        assert!(matches!(text, Cow::Borrowed(_)));
    }

    #[test]
    fn line_text_projects_inlays_after_anchor_characters() {
        let snapshot = snapshot_with("ab\ncd", vec![inlay(1, ": hint")]);
        // 行 0 投影：锚定 'a' 之后注入。
        assert_eq!(snapshot.line_text(Line::new(0)).unwrap(), "a: hintb\n");
        // buffer 行 1 无注入。
        assert_eq!(snapshot.line_text(Line::new(1)).unwrap(), "cd");
    }

    #[test]
    fn multiple_inlays_accumulate_prefix() {
        let snapshot = snapshot_with("ab\ncd", vec![inlay(0, "A"), inlay(1, "BB")]);
        let infos = snapshot.line_inlays(Line::new(0));
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].anchor, 0);
        assert_eq!(infos[0].projected, 0);
        assert_eq!(infos[1].anchor, 1);
        assert_eq!(infos[1].projected, 1 + 1);
        assert_eq!(snapshot.line_text(Line::new(0)).unwrap(), "AaBBb\n");
    }

    #[test]
    fn offset_roundtrip_and_inlay_snapping() {
        let snapshot = snapshot_with("abcdef\n", vec![inlay(2, "XY")]);
        let line = Line::new(0);
        // 原始 → 投影（字符起点语义）：锚定偏移处的字符在注入文本之后，右移注入长度。
        assert_eq!(snapshot.to_projected_offset(line, 1), 1);
        assert_eq!(snapshot.to_projected_offset(line, 2), 4);
        assert_eq!(snapshot.to_projected_offset(line, 3), 5);
        // 投影 → 原始：注入段内吸附到锚定后；段外减去前缀。
        assert_eq!(snapshot.to_original_offset(line, 2), 2);
        assert_eq!(snapshot.to_original_offset(line, 3), 2);
        assert_eq!(snapshot.to_original_offset(line, 4), 2);
        assert_eq!(snapshot.to_original_offset(line, 5), 3);
        assert_eq!(snapshot.to_original_offset(line, 7), 5);
        // roundtrip：原始 → 投影 → 原始 恒等。
        for byte in 0..6 {
            assert_eq!(
                snapshot.to_original_offset(line, snapshot.to_projected_offset(line, byte)),
                byte
            );
        }
    }

    #[test]
    fn version_changes_only_on_inlay_config_change() {
        let buffer = zcv_text::Buffer::scratch("ab\n".to_owned(), BufferConfig::default())
            .expect("测试 Buffer 应能创建");
        let mut map = InlayMap::new(LineStream::new(buffer.snapshot())).0;
        let stream = LineStream::new(buffer.snapshot());
        let snapshot = map.read(stream, vec![inlay(1, "x")]);
        // 相同配置重复读：不变化。
        let snapshot2 = map.read(snapshot.stream().clone(), snapshot.inlays.clone());
        assert_eq!(snapshot2.version(), snapshot.version());
        // 配置变化：版本递增。
        let snapshot3 = map.read(snapshot2.stream().clone(), vec![inlay(1, "xx")]);
        assert_eq!(snapshot3.version(), snapshot.version() + 1);
    }
}
