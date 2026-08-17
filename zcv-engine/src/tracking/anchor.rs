//! Anchor / Mark：可跟随文本变更的位置标记。
//!
//! Anchor 是绑定 `BufferVersion` 的稳定位置；Mark 是不绑定版本的轻量位置标记。
//! 两者都通过 `PositionMap` 更新，不持有 Buffer，也不参与事务提交。

use crate::{
    errors::AnchorError,
    position_map::{Affinity, MappingResult, PositionMap},
    transaction::DeltaEvent,
    types::{BufferVersion, ByteOffset},
};

/// 不绑定 BufferVersion 的轻量位置标记。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mark {
    offset: ByteOffset,
    affinity: Affinity,
}

impl Mark {
    pub fn new(offset: ByteOffset) -> Self {
        Self {
            offset,
            affinity: Affinity::default(),
        }
    }

    pub fn with_affinity(mut self, affinity: Affinity) -> Self {
        self.affinity = affinity;
        self
    }

    pub fn offset(self) -> ByteOffset {
        self.offset
    }

    pub fn affinity(self) -> Affinity {
        self.affinity
    }

    pub fn map_through_position_map(self, position_map: &PositionMap) -> MappingResult<Self> {
        map_mark_result(
            position_map.map_old_position_with_affinity(self.offset, self.affinity),
            self.affinity,
        )
    }
}

impl Default for Mark {
    fn default() -> Self {
        Self::new(ByteOffset::ZERO)
    }
}

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

    pub fn to_mark(self) -> Mark {
        Mark::new(self.offset).with_affinity(self.affinity)
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

fn map_mark_result(result: MappingResult<ByteOffset>, affinity: Affinity) -> MappingResult<Mark> {
    result.map(|offset| Mark::new(offset).with_affinity(affinity))
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
        ChangeSet, Delta, Edit, PositionMap, TransactionId, TransactionSource,
        transaction::EditList,
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
    fn anchor_and_mark_should_map_through_delta_with_affinity() {
        let insert_event = event_for_edits(
            BufferVersion::INITIAL,
            BufferVersion::new(1),
            vec![Edit::insert(b(2), "XX".to_string()).unwrap()],
        );
        let anchor = Anchor::new(BufferVersion::INITIAL, b(2)).with_affinity(Affinity::Before);
        let mark = Mark::new(b(2)).with_affinity(Affinity::After);

        assert_eq!(
            anchor
                .map_through_delta_event(&insert_event)
                .unwrap()
                .value()
                .offset(),
            b(2)
        );
        assert_eq!(
            mark.map_through_position_map(insert_event.position_map())
                .value()
                .offset(),
            b(4)
        );
    }
}
