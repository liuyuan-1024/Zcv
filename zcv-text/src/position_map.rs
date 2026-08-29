//! PositionMap 强类型：把一次文本变更固化为可复用的前后版本坐标映射契约。
//!
//! 本文件只表达 byte offset / text range 在旧文本与新文本之间的映射关系，
//! 不负责 Buffer 提交、事件分发、anchor 生命周期或 UI 选择策略。

use crate::{
    errors::invariant,
    text_changes::TextPatch,
    transaction::Edit,
    types::{ByteOffset, TextRange},
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

impl<T> MappingResult<T> {
    /// 保持变体语义不变，只变换承载值。
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> MappingResult<U> {
        match self {
            Self::Mapped(value) => MappingResult::Mapped(f(value)),
            Self::Deleted(value) => MappingResult::Deleted(f(value)),
            Self::Collapsed(value) => MappingResult::Collapsed(f(value)),
            Self::Ambiguous(value) => MappingResult::Ambiguous(f(value)),
        }
    }
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
}

/// 旧文本与新文本之间的 byte 坐标映射器。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PositionMap {
    edits: Vec<PositionMapEdit>,
}

impl PositionMap {
    /// 从编辑切片构造映射，不持有编辑本身（只投影 range 与长度）。
    pub fn from_edits(edits: &[Edit]) -> Self {
        Self {
            edits: edits
                .iter()
                .map(|edit| PositionMapEdit {
                    old: edit.range(),
                    new_len: edit.replacement().len(),
                })
                .collect(),
        }
    }

    /// 从跨多个连续版本组合后的 Patch 构造坐标映射。
    pub fn from_text_patch(patch: &TextPatch) -> Self {
        Self {
            edits: patch
                .edits()
                .iter()
                .map(|edit| PositionMapEdit {
                    old: edit.old_range(),
                    new_len: edit.new_range().len(),
                })
                .collect(),
        }
    }

    /// old byte position -> new byte position。
    pub fn map_old_position(&self, pos: ByteOffset) -> MappingResult<ByteOffset> {
        self.map_old_position_with_affinity(pos, Affinity::default())
    }

    /// old byte position -> new byte position，显式指定同点插入吸附方向。
    pub fn map_old_position_with_affinity(
        &self,
        pos: ByteOffset,
        affinity: Affinity,
    ) -> MappingResult<ByteOffset> {
        let mut shift = OffsetShift::ZERO;

        for edit in &self.edits {
            let range = edit.old;
            let old_start = range.start();
            let old_end = range.end();
            let replacement_len = edit.new_len;
            let new_start = invariant!(
                shift.apply_old_to_new(old_start),
                "old start 映射不会发生字节偏移溢出"
            );

            if pos < old_start {
                break;
            }

            if old_start == old_end {
                if pos == old_start {
                    let mapped = match affinity {
                        Affinity::Before => new_start,
                        Affinity::After => invariant!(
                            checked_add_offset(new_start, replacement_len),
                            "在 map_old_position_with_affinity 映射时发生字节偏移溢出"
                        ),
                    };

                    return MappingResult::Mapped(mapped);
                }
            } else if pos < old_end {
                return MappingResult::Deleted(new_start);
            }

            shift = invariant!(
                shift.after_edit(range.len(), replacement_len),
                "累计编辑位移不会溢出"
            );
        }

        MappingResult::Mapped(invariant!(
            shift.apply_old_to_new(pos),
            "old position 映射不会发生字节偏移溢出"
        ))
    }

    /// new byte position -> old byte position，显式指定歧义区域偏向。
    pub fn map_new_position_with_bias(
        &self,
        pos: ByteOffset,
        bias: Bias,
    ) -> MappingResult<ByteOffset> {
        let mut shift = OffsetShift::ZERO;

        for edit in &self.edits {
            let range = edit.old;
            let old_start = range.start();
            let old_end = range.end();
            let old_len = range.len();
            let replacement_len = edit.new_len;
            let new_start = invariant!(
                shift.apply_old_to_new(old_start),
                "old start 映射不会发生字节偏移溢出"
            );
            let new_end = invariant!(
                checked_add_offset(new_start, replacement_len),
                "在 map_new_position_with_bias 映射时发生字节偏移溢出"
            );

            if pos < new_start {
                break;
            }

            if replacement_len == 0 {
                if old_len > 0 && pos == new_start {
                    return MappingResult::Ambiguous(biased_offset(old_start, old_end, bias));
                }
            } else if pos < new_end {
                return MappingResult::Ambiguous(biased_offset(old_start, old_end, bias));
            }

            shift = invariant!(
                shift.after_edit(old_len, replacement_len),
                "累计编辑位移不会溢出"
            );
        }

        MappingResult::Mapped(invariant!(
            shift.apply_new_to_old(pos),
            "new position 映射不会发生字节偏移溢出"
        ))
    }

    /// old byte range -> new byte range，显式指定边界处插入文本的吸附 / 扩张策略。
    pub fn map_old_range_with_stickiness(
        &self,
        range: TextRange,
        stickiness: Stickiness,
    ) -> MappingResult<TextRange> {
        let start_affinity = boundary_affinity(stickiness, BoundarySide::Start);
        let end_affinity = boundary_affinity(stickiness, BoundarySide::End);

        // 单遍扫描：一次遍历同时完成起点映射、终点映射与删除区检测，替代原来的三次独立全扫描（map start / map end / intersects deleted）。
        // 每个端点由第一个可能影响它的 edit 决定，语义与逐点映射完全一致。
        let start = range.start();
        let end = range.end();
        let mut shift = OffsetShift::ZERO;
        let mut new_start: Option<ByteOffset> = None;
        let mut new_end: Option<ByteOffset> = None;
        let mut deleted = false;

        for edit in &self.edits {
            let old_start = edit.old.start();
            let old_end = edit.old.end();
            let old_len = edit.old.len();
            let replacement_len = edit.new_len;
            let new_start_at = invariant!(
                shift.apply_old_to_new(old_start),
                "old start 映射不会发生字节偏移溢出"
            );

            if new_start.is_none() {
                if start < old_start {
                    new_start = Some(invariant!(
                        shift.apply_old_to_new(start),
                        "old position 映射不会发生字节偏移溢出"
                    ));
                } else if old_start == old_end {
                    if start == old_start {
                        new_start = Some(match start_affinity {
                            Affinity::Before => new_start_at,
                            Affinity::After => invariant!(
                                checked_add_offset(new_start_at, replacement_len),
                                "在 range 起点映射时发生字节偏移溢出"
                            ),
                        });
                    }
                } else if start < old_end {
                    // 起点落在被删除 / 替换的旧内容中。
                    new_start = Some(new_start_at);
                    deleted = true;
                }
            }

            if new_end.is_none() {
                if end < old_start {
                    new_end = Some(invariant!(
                        shift.apply_old_to_new(end),
                        "old position 映射不会发生字节偏移溢出"
                    ));
                } else if old_start == old_end {
                    if end == old_start {
                        new_end = Some(match end_affinity {
                            Affinity::Before => new_start_at,
                            Affinity::After => invariant!(
                                checked_add_offset(new_start_at, replacement_len),
                                "在 range 终点映射时发生字节偏移溢出"
                            ),
                        });
                    }
                } else if end < old_end {
                    // 终点落在被删除 / 替换的旧内容中。
                    new_end = Some(new_start_at);
                    deleted = true;
                }
            }

            // 删除区检测：非空 edit 的旧区间与目标区间重叠即视为触及删除内容。
            if old_start < old_end && ranges_overlap(start, end, old_start, old_end) {
                deleted = true;
            }

            shift = invariant!(
                shift.after_edit(old_len, replacement_len),
                "累计编辑位移不会溢出"
            );
        }

        let new_start = new_start.unwrap_or_else(|| {
            invariant!(
                shift.apply_old_to_new(start),
                "old position 映射不会发生字节偏移溢出"
            )
        });
        let new_end = new_end.unwrap_or_else(|| {
            invariant!(
                shift.apply_old_to_new(end),
                "old position 映射不会发生字节偏移溢出"
            )
        });
        let (mapped, endpoints_crossed) = text_range_from_mapped_endpoints(new_start, new_end);

        if endpoints_crossed {
            return MappingResult::Collapsed(mapped);
        }

        if !range.is_empty() && mapped.is_empty() {
            return MappingResult::Collapsed(mapped);
        }

        if deleted {
            return MappingResult::Deleted(mapped);
        }

        MappingResult::Mapped(mapped)
    }

    /// 批量映射已排序的 old positions 到 new positions。
    ///
    /// `positions` 必须按非递减顺序排序（由调用方保证）；
    /// 单遍遍历 edits 时用单调指针同时推进所有点，把逐点 O(E) 的映射降为 O(E + P)，用于选区、锚点等批量坐标推进。
    pub fn map_old_positions(
        &self,
        positions: &[ByteOffset],
        affinity: Affinity,
    ) -> Vec<MappingResult<ByteOffset>> {
        let mut results = Vec::with_capacity(positions.len());
        let mut shift = OffsetShift::ZERO;
        let mut next = 0;

        for edit in &self.edits {
            let old_start = edit.old.start();
            let old_end = edit.old.end();
            let old_len = edit.old.len();
            let replacement_len = edit.new_len;
            let new_start_at = invariant!(
                shift.apply_old_to_new(old_start),
                "old start 映射不会发生字节偏移溢出"
            );

            // 当前 edit 之前的所有点：只受累计位移影响。
            while next < positions.len() && positions[next] < old_start {
                results.push(MappingResult::Mapped(invariant!(
                    shift.apply_old_to_new(positions[next]),
                    "old position 映射不会发生字节偏移溢出"
                )));
                next += 1;
            }

            // 落在 edit 起点上的点：同点插入按 affinity 吸附，替换/删除视为 Deleted。
            while next < positions.len() && positions[next] == old_start {
                let result = if old_start == old_end {
                    MappingResult::Mapped(match affinity {
                        Affinity::Before => new_start_at,
                        Affinity::After => invariant!(
                            checked_add_offset(new_start_at, replacement_len),
                            "在批量映射时发生字节偏移溢出"
                        ),
                    })
                } else {
                    MappingResult::Deleted(new_start_at)
                };
                results.push(result);
                next += 1;
            }

            // 落在删除 / 替换区间内部的点：吸附到替换起点。
            while next < positions.len() && positions[next] < old_end {
                results.push(MappingResult::Deleted(new_start_at));
                next += 1;
            }

            shift = invariant!(
                shift.after_edit(old_len, replacement_len),
                "累计编辑位移不会溢出"
            );
        }

        // 剩余点：全部位于最后一个 edit 之后。
        while next < positions.len() {
            results.push(MappingResult::Mapped(invariant!(
                shift.apply_old_to_new(positions[next]),
                "old position 映射不会发生字节偏移溢出"
            )));
            next += 1;
        }

        results
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PositionMapEdit {
    old: TextRange,
    new_len: usize,
}

/// 已应用编辑造成的 byte 坐标位移。
///
/// `removed_bytes` 与 `inserted_bytes` 分开记录，避免把 `usize` 坐标压进
/// `isize`，也避免删除多于插入时出现负数位移再被 clamp 掩盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OffsetShift {
    removed_bytes: usize,
    inserted_bytes: usize,
}

impl OffsetShift {
    pub(crate) const ZERO: Self = Self {
        removed_bytes: 0,
        inserted_bytes: 0,
    };

    pub(crate) fn apply_old_to_new(self, old_offset: ByteOffset) -> Option<ByteOffset> {
        old_offset
            .get()
            .checked_sub(self.removed_bytes)?
            .checked_add(self.inserted_bytes)
            .map(ByteOffset::new)
    }

    pub(crate) fn apply_new_to_old(self, new_offset: ByteOffset) -> Option<ByteOffset> {
        new_offset
            .get()
            .checked_sub(self.inserted_bytes)?
            .checked_add(self.removed_bytes)
            .map(ByteOffset::new)
    }

    pub(crate) fn after_edit(self, old_len: usize, replacement_len: usize) -> Option<Self> {
        Some(Self {
            removed_bytes: self.removed_bytes.checked_add(old_len)?,
            inserted_bytes: self.inserted_bytes.checked_add(replacement_len)?,
        })
    }
}

/// `ByteOffset::checked_add` 的便捷包装。
///
/// 返回 `None` 表示加法后溢出 `usize` 范围。调用方按场景决定是报 `InvariantViolation`
/// 还是按 `MappingResult` 语义静默处理，**本函数不 panic**。
fn checked_add_offset(offset: ByteOffset, rhs: usize) -> Option<ByteOffset> {
    offset.checked_add(rhs)
}

fn biased_offset(start: ByteOffset, end: ByteOffset, bias: Bias) -> ByteOffset {
    match bias {
        Bias::Left => start,
        Bias::Right => end,
    }
}

fn text_range(start: ByteOffset, end: ByteOffset) -> TextRange {
    invariant!(
        TextRange::new(start, end).ok(),
        "PositionMap 生成的 range 必须满足 start <= end"
    )
}

fn text_range_from_mapped_endpoints(start: ByteOffset, end: ByteOffset) -> (TextRange, bool) {
    if start <= end {
        return (text_range(start, end), false);
    }

    (text_range(end, end), true)
}

/// 当前正在计算 stickiness 的区间端点。
///
/// 同一个 `Stickiness` 在起点和终点上的 affinity 往往相反，因此内部映射必须显式区分端点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundarySide {
    /// 区间左边界 / start offset。
    Start,
    /// 区间右边界 / end offset。
    End,
}

pub(crate) fn boundary_affinity(stickiness: Stickiness, side: BoundarySide) -> Affinity {
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
    first_start: ByteOffset,
    first_end: ByteOffset,
    second_start: ByteOffset,
    second_end: ByteOffset,
) -> bool {
    first_start < second_end && second_start < first_end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(value: usize) -> ByteOffset {
        ByteOffset::new(value)
    }

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(b(start), b(end)).unwrap()
    }

    #[test]
    fn position_map_should_expose_affinity_bias_stickiness_and_point_mapping() {
        let map = PositionMap::from_edits(&[Edit::replace(range(1, 3), "XYZ".to_string())]);

        assert!(matches!(map.map_old_position(b(2)), MappingResult::Deleted(pos) if pos == b(1)));
        assert!(matches!(
            map.map_new_position_with_bias(b(2), Bias::Right),
            MappingResult::Ambiguous(pos) if pos == b(3)
        ));
        assert_eq!(
            map.map_old_range_with_stickiness(range(1, 3), Stickiness::Expand)
                .value(),
            range(1, 4)
        );
        assert_eq!(map.map_old_position(b(0)).value(), b(0));
        assert_eq!(map.map_old_position(b(4)).value(), b(5));
    }

    #[test]
    fn position_map_should_not_panic_when_mapped_range_endpoints_cross() {
        let map = PositionMap::from_edits(&[Edit::insert(b(0), "X".to_string()).unwrap()]);

        assert_eq!(
            map.map_old_range_with_stickiness(range(0, 0), Stickiness::Never),
            MappingResult::Collapsed(range(0, 0))
        );
    }
}
