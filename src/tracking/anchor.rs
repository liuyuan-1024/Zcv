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

use super::{AnchorDeletedPolicy, AnchorUpdate};

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
        Ok(self.map_through_position_map(event.new_version, &event.position_map))
    }

    pub fn map_through_delta_event_with_deleted_policy(
        self,
        event: &DeltaEvent,
        deleted_policy: AnchorDeletedPolicy,
    ) -> Result<AnchorUpdate, AnchorError> {
        self.verify_event_version(event)?;
        Ok(self.map_through_position_map_with_deleted_policy(
            event.new_version,
            &event.position_map,
            deleted_policy,
        ))
    }

    pub fn map_through_position_map_with_deleted_policy(
        self,
        new_version: BufferVersion,
        position_map: &PositionMap,
        deleted_policy: AnchorDeletedPolicy,
    ) -> AnchorUpdate {
        match self.map_through_position_map(new_version, position_map) {
            MappingResult::Mapped(anchor) => AnchorUpdate::Mapped(anchor),
            MappingResult::Deleted(anchor) => match deleted_policy {
                AnchorDeletedPolicy::Collapse => AnchorUpdate::Deleted(anchor),
                AnchorDeletedPolicy::Invalidate => AnchorUpdate::Invalidated {
                    mark: anchor.to_mark(),
                    version: new_version,
                },
            },
            MappingResult::Collapsed(anchor) => AnchorUpdate::Deleted(anchor),
            MappingResult::Ambiguous(anchor) => AnchorUpdate::Mapped(anchor),
        }
    }

    pub fn update_through_delta_event(
        &mut self,
        event: &DeltaEvent,
    ) -> Result<MappingResult<Self>, AnchorError> {
        let mapped = self.map_through_delta_event(event)?;
        *self = mapped.value();
        Ok(mapped)
    }

    pub fn update_all_through_delta_event(
        anchors: &mut [Self],
        event: &DeltaEvent,
    ) -> Result<Vec<MappingResult<Self>>, AnchorError> {
        for anchor in anchors.iter().copied() {
            anchor.verify_event_version(event)?;
        }

        let mut updates = Vec::with_capacity(anchors.len());
        for anchor in anchors {
            let mapped = anchor.map_through_position_map(event.new_version, &event.position_map);
            *anchor = mapped.value();
            updates.push(mapped);
        }

        Ok(updates)
    }

    pub fn map_all_through_delta_event_with_deleted_policy(
        anchors: impl IntoIterator<Item = Self>,
        event: &DeltaEvent,
        deleted_policy: AnchorDeletedPolicy,
    ) -> Result<Vec<AnchorUpdate>, AnchorError> {
        anchors
            .into_iter()
            .map(|anchor| anchor.map_through_delta_event_with_deleted_policy(event, deleted_policy))
            .collect()
    }

    fn verify_event_version(self, event: &DeltaEvent) -> Result<(), AnchorError> {
        if self.version != event.old_version {
            return Err(AnchorError::VersionMismatch {
                expected: event.old_version,
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
    match result {
        MappingResult::Mapped(offset) => {
            MappingResult::Mapped(Mark::new(offset).with_affinity(affinity))
        }
        MappingResult::Deleted(offset) => {
            MappingResult::Deleted(Mark::new(offset).with_affinity(affinity))
        }
        MappingResult::Collapsed(offset) => {
            MappingResult::Collapsed(Mark::new(offset).with_affinity(affinity))
        }
        MappingResult::Ambiguous(offset) => {
            MappingResult::Ambiguous(Mark::new(offset).with_affinity(affinity))
        }
    }
}

fn map_anchor_result(
    result: MappingResult<ByteOffset>,
    version: BufferVersion,
    affinity: Affinity,
) -> MappingResult<Anchor> {
    match result {
        MappingResult::Mapped(offset) => {
            MappingResult::Mapped(Anchor::new(version, offset).with_affinity(affinity))
        }
        MappingResult::Deleted(offset) => {
            MappingResult::Deleted(Anchor::new(version, offset).with_affinity(affinity))
        }
        MappingResult::Collapsed(offset) => {
            MappingResult::Collapsed(Anchor::new(version, offset).with_affinity(affinity))
        }
        MappingResult::Ambiguous(offset) => {
            MappingResult::Ambiguous(Anchor::new(version, offset).with_affinity(affinity))
        }
    }
}
