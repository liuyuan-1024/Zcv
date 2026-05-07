//! PositionMap 强类型：把一次文本变更固化为可复用的前后版本坐标映射契约。
//!
//! 本文件只表达 char offset / text range 在旧文本与新文本之间的映射关系，
//! 不负责 Buffer 提交、事件分发、anchor 生命周期或 UI 选择策略。

use crate::{
    selection::{Selection, SelectionSet},
    tracking::{TrackedRange, TrackedRangeUpdate, TrackedRangeUpdatePolicy},
    transaction::{ChangeSet, Edit, EditList},
    types::{BufferVersion, CharOffset, TextRange},
};

/// 同点插入时旧位置吸附到插入文本前还是插入文本后。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Affinity {
    /// 吸附到插入文本之前。
    Before,
    /// 吸附到插入文本之后。
    #[default]
    After,
}

/// 反向映射遇到歧义时选择偏左还是偏右的旧文本落点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Bias {
    /// 选择歧义区域左侧 / 起点。
    #[default]
    Left,
    /// 选择歧义区域右侧 / 终点。
    Right,
}

/// 旧区间边界遇到同点插入时的扩张策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Stickiness {
    /// 区间边界吸附在插入文本之前。
    BeforeInsertion,
    /// 区间边界吸附在插入文本之后。
    AfterInsertion,
    /// 两侧边界都向外扩张，纳入边界处插入的新文本。
    Expand,
    /// 两侧边界都向内收缩，不纳入边界处插入的新文本。
    #[default]
    Never,
}

/// 坐标或区间映射结果。
///
/// payload 是按当前方向得到的最佳落点；调用方可以根据 variant 决定是继续使用、
/// 失效、塌缩，还是交给更高层的 affinity / stickiness 策略处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MappingResult<T> {
    /// 源坐标可以无歧义地映射到目标坐标。
    Mapped(T),
    /// 源坐标落在被删除或被替换的旧内容中。
    Deleted(T),
    /// 源区间被删除后塌缩为零宽区间。
    Collapsed(T),
    /// 目标坐标落在插入或替换产生的新内容中，无法唯一反推旧坐标。
    Ambiguous(T),
}

impl<T: Copy> MappingResult<T> {
    pub fn value(self) -> T {
        match self {
            Self::Mapped(value)
            | Self::Deleted(value)
            | Self::Collapsed(value)
            | Self::Ambiguous(value) => value,
        }
    }

    pub fn is_mapped(self) -> bool {
        matches!(self, Self::Mapped(_))
    }
}

/// 旧文本与新文本之间的 char 坐标映射器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionMap {
    edits: Vec<Edit>,
}

impl PositionMap {
    pub fn from_edit_list(edits: &EditList) -> Self {
        Self {
            edits: edits.as_slice().to_vec(),
        }
    }

    pub(crate) fn from_edits(edits: Vec<Edit>) -> Self {
        Self { edits }
    }

    pub fn from_change_set(changeset: &ChangeSet) -> Self {
        Self::from_edits(changeset.edits().to_vec())
    }

    pub fn len(&self) -> usize {
        self.edits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// old char position -> new char position。
    pub fn map_old_position(&self, pos: CharOffset) -> MappingResult<CharOffset> {
        self.map_old_position_with_affinity(pos, Affinity::default())
    }

    /// old char position -> new char position，显式指定同点插入吸附方向。
    pub fn map_old_position_with_affinity(
        &self,
        pos: CharOffset,
        affinity: Affinity,
    ) -> MappingResult<CharOffset> {
        let mut diff = 0isize;
        let pos_val = pos.get() as isize;

        for edit in &self.edits {
            let old_start = edit.range.start().get() as isize;
            let old_end = edit.range.end().get() as isize;
            let replacement_len = replacement_len(edit);

            if pos_val < old_start {
                break;
            }

            if old_start == old_end {
                if pos_val == old_start {
                    let mapped = match affinity {
                        Affinity::Before => old_start + diff,
                        Affinity::After => old_start + diff + replacement_len,
                    };

                    return MappingResult::Mapped(offset(mapped));
                }
            } else if pos_val < old_end {
                return MappingResult::Deleted(offset(old_start + diff));
            }

            diff += replacement_len - (old_end - old_start);
        }

        MappingResult::Mapped(offset(pos_val + diff))
    }

    /// new char position -> old char position。
    pub fn map_new_position(&self, pos: CharOffset) -> MappingResult<CharOffset> {
        self.map_new_position_with_bias(pos, Bias::default())
    }

    /// new char position -> old char position，显式指定歧义区域偏向。
    pub fn map_new_position_with_bias(
        &self,
        pos: CharOffset,
        bias: Bias,
    ) -> MappingResult<CharOffset> {
        let mut diff = 0isize;
        let pos_val = pos.get() as isize;

        for edit in &self.edits {
            let old_start = edit.range.start().get() as isize;
            let old_end = edit.range.end().get() as isize;
            let old_len = old_end - old_start;
            let replacement_len = replacement_len(edit);
            let new_start = old_start + diff;
            let new_end = new_start + replacement_len;

            if pos_val < new_start {
                break;
            }

            if replacement_len == 0 {
                if old_len > 0 && pos_val == new_start {
                    return MappingResult::Ambiguous(biased_offset(old_start, old_end, bias));
                }
            } else if pos_val < new_end {
                return MappingResult::Ambiguous(biased_offset(old_start, old_end, bias));
            }

            diff += replacement_len - old_len;
        }

        MappingResult::Mapped(offset(pos_val - diff))
    }

    /// old char range -> new char range。
    pub fn map_old_range(&self, range: TextRange) -> MappingResult<TextRange> {
        self.map_old_range_with_stickiness(range, Stickiness::default())
    }

    /// old char range -> new char range，显式指定边界处插入文本的吸附 / 扩张策略。
    pub fn map_old_range_with_stickiness(
        &self,
        range: TextRange,
        stickiness: Stickiness,
    ) -> MappingResult<TextRange> {
        let new_start = self
            .map_old_position_with_affinity(
                range.start(),
                boundary_affinity(stickiness, BoundarySide::Start),
            )
            .value();
        let new_end = self
            .map_old_position_with_affinity(
                range.end(),
                boundary_affinity(stickiness, BoundarySide::End),
            )
            .value();
        let mapped = text_range(new_start, new_end);

        if !range.is_empty() && mapped.is_empty() {
            return MappingResult::Collapsed(mapped);
        }

        if self.old_range_intersects_deleted_content(range) {
            return MappingResult::Deleted(mapped);
        }

        MappingResult::Mapped(mapped)
    }

    /// new char range -> old char range。
    pub fn map_new_range(&self, range: TextRange) -> MappingResult<TextRange> {
        let (old_start, old_end) = if range.is_empty() {
            let position = self.map_new_position(range.start()).value();
            (position, position)
        } else {
            (
                self.map_new_position_for_range_boundary(range.start(), true),
                self.map_new_position_for_range_boundary(range.end(), false),
            )
        };
        let mapped = text_range(old_start, old_end);

        if self.new_range_touches_ambiguous_content(range) {
            return MappingResult::Ambiguous(mapped);
        }

        MappingResult::Mapped(mapped)
    }

    /// old Selection -> new Selection。
    pub fn map_selection(&self, selection: Selection) -> Selection {
        selection.map_through_position_map(self)
    }

    /// old SelectionSet -> new SelectionSet。
    pub fn map_selection_set(&self, selection_set: &SelectionSet) -> SelectionSet {
        selection_set.map_through_position_map(self)
    }

    /// old TrackedRange -> new TrackedRange。
    pub fn map_tracked_range(
        &self,
        tracked_range: TrackedRange,
        new_version: BufferVersion,
    ) -> MappingResult<TrackedRange> {
        tracked_range.map_through_position_map(new_version, self)
    }

    /// old TrackedRange -> new TrackedRange，并应用删除 / 塌缩失效策略。
    pub fn map_tracked_range_with_policy(
        &self,
        tracked_range: TrackedRange,
        new_version: BufferVersion,
        policy: TrackedRangeUpdatePolicy,
    ) -> TrackedRangeUpdate {
        tracked_range.map_through_position_map_with_policy(new_version, self, policy)
    }

    /// 批量映射 TrackedRange。PositionMap 本身不绑定版本，调用方显式传入目标版本。
    pub fn map_tracked_ranges(
        &self,
        tracked_ranges: impl IntoIterator<Item = TrackedRange>,
        new_version: BufferVersion,
    ) -> Vec<MappingResult<TrackedRange>> {
        tracked_ranges
            .into_iter()
            .map(|tracked_range| self.map_tracked_range(tracked_range, new_version))
            .collect()
    }

    /// 批量映射 TrackedRange，并应用同一删除 / 塌缩失效策略。
    pub fn map_tracked_ranges_with_policy(
        &self,
        tracked_ranges: impl IntoIterator<Item = TrackedRange>,
        new_version: BufferVersion,
        policy: TrackedRangeUpdatePolicy,
    ) -> Vec<TrackedRangeUpdate> {
        tracked_ranges
            .into_iter()
            .map(|tracked_range| {
                self.map_tracked_range_with_policy(tracked_range, new_version, policy)
            })
            .collect()
    }

    fn map_new_position_for_range_boundary(
        &self,
        pos: CharOffset,
        use_after_deleted_content: bool,
    ) -> CharOffset {
        let mut diff = 0isize;
        let pos_val = pos.get() as isize;

        for edit in &self.edits {
            let old_start = edit.range.start().get() as isize;
            let old_end = edit.range.end().get() as isize;
            let old_len = old_end - old_start;
            let replacement_len = replacement_len(edit);
            let new_start = old_start + diff;
            let new_end = new_start + replacement_len;

            if pos_val < new_start {
                break;
            }

            if replacement_len == 0 {
                if old_len > 0 && pos_val == new_start {
                    return if use_after_deleted_content {
                        offset(old_end)
                    } else {
                        offset(old_start)
                    };
                }
            } else if pos_val < new_end {
                return offset(old_start);
            }

            diff += replacement_len - old_len;
        }

        offset(pos_val - diff)
    }

    fn old_range_intersects_deleted_content(&self, range: TextRange) -> bool {
        self.edits.iter().any(|edit| {
            let old_start = edit.range.start();
            let old_end = edit.range.end();

            old_start < old_end && ranges_overlap(range.start(), range.end(), old_start, old_end)
        })
    }

    fn new_range_touches_ambiguous_content(&self, range: TextRange) -> bool {
        let mut diff = 0isize;

        for edit in &self.edits {
            let old_start = edit.range.start().get() as isize;
            let old_end = edit.range.end().get() as isize;
            let old_len = old_end - old_start;
            let replacement_len = replacement_len(edit);
            let new_start = offset(old_start + diff);
            let new_end = offset(old_start + diff + replacement_len);

            if replacement_len == 0 {
                let touches_deleted_point = if range.is_empty() {
                    range.start() == new_start
                } else {
                    range.start() < new_start && new_start < range.end()
                };

                if old_len > 0 && touches_deleted_point {
                    return true;
                }
            } else if range_touches_span(range, new_start, new_end) {
                return true;
            }

            diff += replacement_len - old_len;
        }

        false
    }
}

fn replacement_len(edit: &Edit) -> isize {
    edit.replacement.chars().count() as isize
}

fn offset(value: isize) -> CharOffset {
    CharOffset::new(value.max(0) as usize)
}

fn biased_offset(start: isize, end: isize, bias: Bias) -> CharOffset {
    match bias {
        Bias::Left => offset(start),
        Bias::Right => offset(end),
    }
}

fn text_range(start: CharOffset, end: CharOffset) -> TextRange {
    TextRange::new(start, end).expect("PositionMap 生成的 range 必须满足 start <= end")
}

/// 当前正在计算 stickiness 的区间端点。
///
/// 同一个 `Stickiness` 在起点和终点上的 affinity 往往相反，因此内部映射必须显式区分端点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundarySide {
    /// 区间左边界 / start offset。
    Start,
    /// 区间右边界 / end offset。
    End,
}

fn boundary_affinity(stickiness: Stickiness, side: BoundarySide) -> Affinity {
    match stickiness {
        Stickiness::BeforeInsertion => Affinity::Before,
        Stickiness::AfterInsertion => Affinity::After,
        Stickiness::Expand => match side {
            BoundarySide::Start => Affinity::Before,
            BoundarySide::End => Affinity::After,
        },
        Stickiness::Never => match side {
            BoundarySide::Start => Affinity::After,
            BoundarySide::End => Affinity::Before,
        },
    }
}

fn ranges_overlap(
    first_start: CharOffset,
    first_end: CharOffset,
    second_start: CharOffset,
    second_end: CharOffset,
) -> bool {
    first_start < second_end && second_start < first_end
}

fn range_touches_span(range: TextRange, span_start: CharOffset, span_end: CharOffset) -> bool {
    if range.is_empty() {
        span_start <= range.start() && range.start() < span_end
    } else {
        ranges_overlap(range.start(), range.end(), span_start, span_end)
    }
}
