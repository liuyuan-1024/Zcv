use super::*;

impl Editor {
    pub(crate) fn search_highlights(&self) -> Option<(&[SearchMatchAnchor], usize)> {
        let search = self.search.as_ref()?;
        if search.len() == 0 {
            return None;
        }
        Some((search.matches(), search.active_index.unwrap_or(0)))
    }
}
