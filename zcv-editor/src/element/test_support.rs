//! 编辑器元素布局的测试专用访问器。
//!
//! 生产命中测试只消费显示坐标的逆向映射；
//! 源偏移到窗口像素位置的正向映射仅供测试计算点击坐标使用。

use super::*;

impl EditorInputLayout {
    /// 源偏移在当前帧布局中的窗口像素位置。
    pub(crate) fn caret_position_for_offset(
        &self,
        offset: MultiBufferOffset,
    ) -> Option<Point<Pixels>> {
        let display_point = self
            .layout
            .display_snapshot
            .offset_to_display_point(offset)
            .ok()?;
        let line = self
            .layout
            .lines
            .iter()
            .find(|line| line.row == display_point.row())?;
        let local_byte =
            local_byte_for_display_point(line, display_point, &self.layout.display_snapshot);
        Some(point(
            line.origin.x + line.line.x_for_index(local_byte),
            line.origin.y,
        ))
    }

    /// 当前帧布局的行高。
    pub(crate) fn line_height(&self) -> Pixels {
        self.layout.line_height
    }
}
