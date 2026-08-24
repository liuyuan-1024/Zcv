//! `VersionedResult<T>`：把任意 payload 与 `BufferVersion` 绑定的通用载体。
//!
//! 本模块只表达版本绑定、过期判断和通过 `PositionMap` 尝试 remap 的边界；
//! 不携带任何业务 payload 语义，具体的 remap 行为由调用方在闭包里完成。

use crate::{
    TextResult, errors::VersionedResultError, position_map::PositionMap, transaction::DeltaEvent,
    types::BufferVersion,
};

/// 与 `BufferVersion` 绑定的泛型结果载体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VersionedResult<T> {
    version: BufferVersion,
    value: T,
}

impl<T> VersionedResult<T> {
    /// 绑定结果到 `BufferVersion`，用于消费侧校验版本一致性。
    pub const fn new(version: BufferVersion, value: T) -> Self {
        Self { version, value }
    }

    /// 结果绑定的 BufferVersion。
    pub fn version(&self) -> BufferVersion {
        self.version
    }

    /// 只读 payload。
    pub fn value(&self) -> &T {
        &self.value
    }

    /// 当前结果是否相对 `current` 版本已过期。
    pub fn is_stale(&self, current: BufferVersion) -> bool {
        self.version != current
    }

    /// 通过一次 `DeltaEvent` 把 payload 推进到新版本。
    ///
    /// `event.old_version()` 必须与当前结果版本一致，否则原子拒绝、不调用 `remap`。
    /// 闭包接收 payload by value 与 `&PositionMap`，返回新 payload 或失败原因。
    pub fn try_remap<F>(self, event: &DeltaEvent, remap: F) -> TextResult<VersionedResult<T>>
    where
        F: FnOnce(T, &PositionMap) -> Result<T, VersionedResultError>,
    {
        if event.old_version() != self.version {
            return Err(VersionedResultError::VersionMismatch {
                expected: self.version,
                actual: event.old_version(),
            }
            .into());
        }

        let new_value = remap(self.value, event.position_map())?;
        Ok(VersionedResult::new(event.new_version(), new_value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ByteOffset, ChangeSet, Delta, Edit, PositionMap, TextError, TextRange, TransactionId,
        TransactionSource, transaction::EditList,
    };

    fn b(value: usize) -> ByteOffset {
        ByteOffset::new(value)
    }

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(b(start), b(end)).unwrap()
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
    fn versioned_result_should_reject_stale_delta_and_remap_payload_through_position_map() {
        let event = event_for_edits(
            BufferVersion::INITIAL,
            BufferVersion::new(1),
            vec![Edit::insert(b(0), "XX".to_string()).unwrap()],
        );
        let result = VersionedResult::new(BufferVersion::INITIAL, range(2, 5));

        let remapped = result
            .try_remap(&event, |range, map| {
                Ok(map
                    .map_old_range_with_stickiness(range, crate::Stickiness::default())
                    .value())
            })
            .unwrap();

        assert_eq!(remapped.version(), event.new_version());
        assert_eq!(*remapped.value(), range(4, 7));

        let stale = VersionedResult::new(BufferVersion::new(42), range(0, 1));
        let err = stale
            .try_remap(&event, |range, map| {
                Ok(map
                    .map_old_range_with_stickiness(range, crate::Stickiness::default())
                    .value())
            })
            .unwrap_err();
        assert!(matches!(
            err,
            TextError::Versioned(VersionedResultError::VersionMismatch { .. })
        ));
    }
}
