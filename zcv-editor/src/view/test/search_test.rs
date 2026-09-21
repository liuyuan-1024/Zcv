use super::*;

impl Editor {
    pub(crate) fn search_highlights(&self) -> Option<(&[MultiBufferRange], usize)> {
        let search = self.search.as_ref()?;
        if search.ranges.is_empty() {
            return None;
        }
        Some((search.ranges.as_ref(), search.active_index.unwrap_or(0)))
    }

    /// 搜索状态是否仍是宿主注入的外部派生结果集。
    pub(crate) fn search_result_is_external(&self) -> bool {
        self.search
            .as_ref()
            .is_some_and(|search| matches!(search.result, Some(SearchResultKind::External { .. })))
    }
}
