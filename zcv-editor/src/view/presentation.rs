//! Editor 的统一文本展示快照。
//!
//! 展示快照把源文本快照与输入法标记、重命名淡化范围组合起来，供布局层消费；
//! 它不拥有文档内容或任何编辑会话状态。

use zcv_multi_buffer::MultiBufferRange;

use std::ops::Range;
use std::sync::Arc;

use zcv_multi_buffer::MultiBufferSnapshot;
use zcv_text::Utf16Offset;

use super::input::EditorComposition;

#[derive(Debug, Clone)]
pub(crate) struct EditorPresentation {
    snapshot: MultiBufferSnapshot,
    composition: Option<EditorComposition>,
    dimmed_ranges: Arc<[Range<usize>]>,
}

impl EditorPresentation {
    pub(crate) fn new(
        snapshot: &MultiBufferSnapshot,
        composition: Option<&EditorComposition>,
    ) -> Self {
        Self {
            snapshot: snapshot.clone(),
            composition: composition.cloned(),
            dimmed_ranges: Arc::from([]),
        }
    }

    pub(crate) fn with_dimmed_ranges(mut self, ranges: Arc<[Range<usize>]>) -> Self {
        self.dimmed_ranges = ranges;
        self
    }

    pub(crate) fn dimmed_ranges(&self) -> &[Range<usize>] {
        &self.dimmed_ranges
    }

    pub(crate) fn marked_ranges(&self) -> &[MultiBufferRange] {
        self.composition
            .as_ref()
            .map_or(&[], |composition| composition.ranges.as_ref())
    }

    pub(crate) fn marked_utf16_range(&self) -> Option<Range<usize>> {
        let composition = self.composition.as_ref()?;
        let range = composition.ranges.get(composition.primary_index)?;
        Some(
            self.snapshot.byte_to_utf16_cu(range.start()).ok()?.get()
                ..self.snapshot.byte_to_utf16_cu(range.end()).ok()?.get(),
        )
    }

    pub(crate) fn text_for_utf16_range(&self, range: Range<usize>) -> Option<String> {
        let start = self
            .snapshot
            .utf16_cu_to_byte(Utf16Offset::new(range.start))
            .ok()?;
        let end = self
            .snapshot
            .utf16_cu_to_byte(Utf16Offset::new(range.end))
            .ok()?;
        Some(
            self.snapshot
                .bytes_in_range(start..end)
                .map(|chunk| chunk.text)
                .collect(),
        )
    }
}
