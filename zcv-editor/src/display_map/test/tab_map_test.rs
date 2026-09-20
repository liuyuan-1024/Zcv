use super::*;

impl TabMap {
    pub(crate) fn measured_lines(&self) -> impl Iterator<Item = (Line, TabColumn)> + '_ {
        self.measured_line_widths
            .iter()
            .map(|(line, width)| (*line, *width))
    }
}
