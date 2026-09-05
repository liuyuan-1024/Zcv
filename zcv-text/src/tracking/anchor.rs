//! Anchor：绑定 `BufferVersion` 的稳定位置标记。
//! 通过 `PositionMap` 更新，不持有 Buffer，也不参与事务提交。

use std::ops::Range;

use crate::{
    errors::AnchorError,
    position_map::{Affinity, MappingResult, PositionMap},
    transaction::DeltaEvent,
    types::{BufferVersion, ByteOffset, TextRange},
};

/// 绑定 BufferVersion 的稳定位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Anchor {
    version: BufferVersion,
    offset: ByteOffset,
    affinity: Affinity,
}

impl Anchor {
    pub fn new(version: BufferVersion, offset: ByteOffset) -> Self {
        Self {
            version,
            offset,
            affinity: Affinity::default(),
        }
    }

    pub fn with_affinity(mut self, affinity: Affinity) -> Self {
        self.affinity = affinity;
        self
    }

    pub fn version(self) -> BufferVersion {
        self.version
    }

    pub fn offset(self) -> ByteOffset {
        self.offset
    }

    pub fn affinity(self) -> Affinity {
        self.affinity
    }

    /// 创建一个不吸收边界插入的锚点范围。
    ///
    /// 起点贴在插入内容之后，终点贴在插入内容之前；
    /// 适合折叠、diff hunk 等只跟随原有文本而不扩张的范围。
    pub fn range_inside(version: BufferVersion, range: TextRange) -> Range<Self> {
        Self::new(version, range.start()).with_affinity(Affinity::After)
            ..Self::new(version, range.end()).with_affinity(Affinity::Before)
    }

    /// 创建一个吸收边界插入的锚点范围。
    ///
    /// 起点贴在插入内容之前，终点贴在插入内容之后。
    /// 适合自动闭合对这类需要把内部新输入继续纳入范围的标记。
    pub fn range_outside(version: BufferVersion, range: TextRange) -> Range<Self> {
        Self::new(version, range.start()).with_affinity(Affinity::Before)
            ..Self::new(version, range.end()).with_affinity(Affinity::After)
    }

    pub fn map_through_position_map(
        self,
        new_version: BufferVersion,
        position_map: &PositionMap,
    ) -> MappingResult<Self> {
        map_anchor_result(
            position_map.map_old_position_with_affinity(self.offset, self.affinity),
            new_version,
            self.affinity,
        )
    }

    pub fn map_through_delta_event(
        self,
        event: &DeltaEvent,
    ) -> Result<MappingResult<Self>, AnchorError> {
        self.verify_event_version(event)?;
        Ok(self.map_through_position_map(event.new_version(), event.position_map()))
    }

    pub fn update_through_delta_event(
        &mut self,
        event: &DeltaEvent,
    ) -> Result<MappingResult<Self>, AnchorError> {
        let mapped = self.map_through_delta_event(event)?;
        *self = mapped.value();
        Ok(mapped)
    }

    fn verify_event_version(self, event: &DeltaEvent) -> Result<(), AnchorError> {
        if self.version != event.old_version() {
            return Err(AnchorError::VersionMismatch {
                expected: event.old_version(),
                actual: self.version,
            });
        }

        Ok(())
    }
}

impl Default for Anchor {
    fn default() -> Self {
        Self::new(BufferVersion::INITIAL, ByteOffset::ZERO)
    }
}

fn map_anchor_result(
    result: MappingResult<ByteOffset>,
    version: BufferVersion,
    affinity: Affinity,
) -> MappingResult<Anchor> {
    result.map(|offset| Anchor::new(version, offset).with_affinity(affinity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        position_map::PositionMap,
        transaction::{ChangeSet, Delta, Edit, EditList, TransactionSource},
        types::TransactionId,
    };

    fn b(value: usize) -> ByteOffset {
        ByteOffset::new(value)
    }

    fn event_for_edits(
        old_version: BufferVersion,
        new_version: BufferVersion,
        edits: Vec<Edit>,
    ) -> DeltaEvent {
        let edit_list = EditList::new(edits).unwrap();
        let delta = Delta::new(old_version, new_version, edit_list.clone());
        let changeset = ChangeSet::from_edit_list(&edit_list);
        let position_map = PositionMap::from_edits(edit_list.as_slice());

        DeltaEvent::new(
            TransactionId::INITIAL,
            TransactionSource::Programmatic,
            delta,
            changeset,
            position_map,
        )
    }

    #[test]
    fn anchor_should_map_through_delta_with_affinity() {
        let insert_event = event_for_edits(
            BufferVersion::INITIAL,
            BufferVersion::new(1),
            vec![Edit::insert(b(2), "XX".to_string()).unwrap()],
        );
        let anchor = Anchor::new(BufferVersion::INITIAL, b(2)).with_affinity(Affinity::Before);

        assert_eq!(
            anchor
                .map_through_delta_event(&insert_event)
                .unwrap()
                .value()
                .offset(),
            b(2)
        );
    }

    #[test]
    fn anchor_ranges_should_express_boundary_insertion_policy() {
        let range = TextRange::new(b(2), b(5)).unwrap();
        let inside = Anchor::range_inside(BufferVersion::INITIAL, range);
        let outside = Anchor::range_outside(BufferVersion::INITIAL, range);

        assert_eq!(inside.start.affinity(), Affinity::After);
        assert_eq!(inside.end.affinity(), Affinity::Before);
        assert_eq!(outside.start.affinity(), Affinity::Before);
        assert_eq!(outside.end.affinity(), Affinity::After);
    }
}
