use super::*;

impl Editor {
    pub(crate) fn search_highlights(&self) -> Option<(&[MultiBufferRange], usize)> {
        let search = self.search.as_ref()?;
        if search.ranges.is_empty() {
            return None;
        }
        Some((search.ranges.as_ref(), search.active_index.unwrap_or(0)))
    }
}
