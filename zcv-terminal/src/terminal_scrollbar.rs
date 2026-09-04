//! 终端回看记录到通用滚动条坐标的适配。

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui::{Bounds, Pixels, Point, point, px, size};
use zcv_ui::ScrollableHandle;

use crate::Content;

#[derive(Debug)]
struct ScrollHandleState {
    line_height: Pixels,
    total_lines: usize,
    viewport_lines: usize,
    display_offset: usize,
}

impl ScrollHandleState {
    fn empty() -> Self {
        Self {
            line_height: px(1.),
            total_lines: 0,
            viewport_lines: 0,
            display_offset: 0,
        }
    }

    fn from_content(content: &Content) -> Self {
        Self {
            line_height: content.terminal_bounds.line_height(),
            total_lines: content.total_lines,
            viewport_lines: content.screen_lines,
            display_offset: content.display_offset,
        }
    }

    fn max_offset_lines(&self) -> usize {
        self.total_lines.saturating_sub(self.viewport_lines)
    }
}

/// 将滚动条拖动请求延迟到下一帧终端同步时应用，避免渲染层直接触碰终端模拟器。
#[derive(Clone)]
pub(crate) struct TerminalScrollHandle {
    state: Rc<RefCell<ScrollHandleState>>,
    requested_display_offset: Rc<Cell<Option<usize>>>,
}

impl TerminalScrollHandle {
    pub(crate) fn new() -> Self {
        Self {
            state: Rc::new(RefCell::new(ScrollHandleState::empty())),
            requested_display_offset: Rc::new(Cell::new(None)),
        }
    }

    pub(crate) fn update(&self, content: Option<&Content>) {
        *self.state.borrow_mut() =
            content.map_or_else(ScrollHandleState::empty, ScrollHandleState::from_content);
    }

    pub(crate) fn take_requested_display_offset(&self) -> Option<usize> {
        self.requested_display_offset.take()
    }
}

impl ScrollableHandle for TerminalScrollHandle {
    fn max_offset(&self) -> Point<Pixels> {
        let state = self.state.borrow();
        point(
            Pixels::ZERO,
            state.max_offset_lines() as f32 * state.line_height,
        )
    }

    fn offset(&self) -> Point<Pixels> {
        let state = self.state.borrow();
        let offset_from_top = state
            .max_offset_lines()
            .saturating_sub(state.display_offset);
        point(Pixels::ZERO, -(offset_from_top as f32 * state.line_height))
    }

    fn set_offset(&self, point: Point<Pixels>) {
        let state = self.state.borrow();
        let offset_delta = (point.y / state.line_height).round() as i32;
        let max_offset = state.max_offset_lines();
        let display_offset = (max_offset as i32 + offset_delta).clamp(0, max_offset as i32);
        self.requested_display_offset
            .set(Some(display_offset as usize));
    }

    fn viewport(&self) -> Bounds<Pixels> {
        let state = self.state.borrow();
        Bounds::new(
            point(Pixels::ZERO, Pixels::ZERO),
            size(
                Pixels::ZERO,
                state.viewport_lines as f32 * state.line_height,
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_the_thumb_targets_the_matching_history_offset() {
        let handle = TerminalScrollHandle {
            state: Rc::new(RefCell::new(ScrollHandleState {
                line_height: px(16.),
                total_lines: 30,
                viewport_lines: 10,
                display_offset: 4,
            })),
            requested_display_offset: Rc::new(Cell::new(None)),
        };

        assert_eq!(handle.max_offset().y, px(320.));
        assert_eq!(handle.offset().y, px(-256.));

        handle.set_offset(point(px(0.), px(-80.)));
        assert_eq!(handle.take_requested_display_offset(), Some(15));
    }
}
