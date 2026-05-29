//! Dock resize 交互。
//!
//! 本模块定义 Workbench 内部 resize 事件、回调类型，并集中处理拖拽过程中的
//! 尺寸计算。具体 dock 只负责发事件，controller 只负责读写对应 Dock 状态。

use std::rc::Rc;

use gpui::{
    AnyElement, App, EmptyView, MouseButton, Pixels, Point, Window, deferred, div, prelude::*, px,
};

use crate::shell::shared::theme;

use super::super::state::DockAreaId;

const HANDLE_SIZE: Pixels = px(6.0);
const HANDLE_OVERLAP: Pixels = px(-3.0);

#[derive(Clone, Copy)]
pub(crate) enum DockResizeEvent {
    Start {
        area: DockAreaId,
        position: Point<Pixels>,
    },
    Drag {
        position: Point<Pixels>,
    },
    End,
}

pub(in crate::shell::workbench) type DockResizeRequest =
    Rc<dyn Fn(DockResizeEvent, &mut Window, &mut App)>;

#[derive(Clone, Copy)]
pub(crate) struct DockResizeBounds {
    pub(crate) width: Pixels,
    pub(crate) body_height: Pixels,
}

impl DockResizeBounds {
    pub(crate) fn from_viewport(viewport_width: Pixels, viewport_height: Pixels) -> Self {
        // bar_height：UI 行高 + 上下内边距 + 与 body 交界处那根 1px 分隔线
        let bar_height = theme::typography::ui_line() + theme::space::s4() * 2.0 + px(1.0);

        Self {
            width: viewport_width,
            body_height: viewport_height - bar_height * 2.0,
        }
    }
}

#[derive(Clone, Copy)]
struct DockResizeHandleDrag {
    area: DockAreaId,
}

#[derive(Clone, Copy)]
struct DockResizeDrag {
    area: DockAreaId,
    start_position: Point<Pixels>,
    start_size: Pixels,
}

#[derive(Clone, Copy)]
pub(crate) struct DockResizeUpdate {
    pub(crate) area: DockAreaId,
    pub(crate) size: Pixels,
}

#[derive(Default)]
pub(crate) struct DockResize {
    active_drag: Option<DockResizeDrag>,
}

impl DockResize {
    pub(crate) fn handle(
        &mut self,
        event: DockResizeEvent,
        start_size: Option<Pixels>,
        bounds: DockResizeBounds,
    ) -> Option<DockResizeUpdate> {
        match event {
            DockResizeEvent::Start { area, position } => {
                self.start(area, position, start_size);
                None
            }
            DockResizeEvent::Drag { position } => self.resize(position, bounds),
            DockResizeEvent::End => {
                self.end();
                None
            }
        }
    }

    fn start(&mut self, area: DockAreaId, position: Point<Pixels>, start_size: Option<Pixels>) {
        let Some(start_size) = start_size else {
            return;
        };
        self.active_drag = Some(DockResizeDrag {
            area,
            start_position: position,
            start_size,
        });
    }

    fn resize(
        &mut self,
        position: Point<Pixels>,
        bounds: DockResizeBounds,
    ) -> Option<DockResizeUpdate> {
        let drag = self.active_drag?;
        let delta = position - drag.start_position;
        let (size, max_extent) = match drag.area {
            DockAreaId::Left => (drag.start_size + delta.x, bounds.width),
            DockAreaId::Right => (drag.start_size - delta.x, bounds.width),
            DockAreaId::Bottom => (drag.start_size - delta.y, bounds.body_height),
        };
        Some(DockResizeUpdate {
            area: drag.area,
            size: clamp_size(size, max_extent),
        })
    }

    fn end(&mut self) {
        self.active_drag = None;
    }
}

/// 每个 dock 拖到极限时，对侧保留一段恰好等于该 dock 最小尺寸的空白：
/// 三个 dock 共用 `s12`，正好是「比拖拽手柄宽一点的最窄可视带」，既挡住
/// BottomDock 顶死 EditorGrid 上边界，又留出可抓的拖拽区域。
fn clamp_size(size: Pixels, max_extent: Pixels) -> Pixels {
    let min_size = theme::space::s12();
    let max_size = (max_extent - min_size).max(min_size);
    size.clamp(min_size, max_size)
}

pub(crate) fn render_handle(area: DockAreaId, resize: DockResizeRequest) -> AnyElement {
    let mut handle = div()
        .id(handle_id(area))
        .absolute()
        .on_mouse_down(MouseButton::Left, {
            let resize = resize.clone();
            move |event, window, cx| {
                resize(
                    DockResizeEvent::Start {
                        area,
                        position: event.position,
                    },
                    window,
                    cx,
                );
                cx.stop_propagation();
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let resize = resize.clone();
            move |_, window, cx| {
                resize(DockResizeEvent::End, window, cx);
                cx.stop_propagation();
            }
        })
        .on_mouse_up_out(MouseButton::Left, {
            let resize = resize.clone();
            move |_, window, cx| {
                resize(DockResizeEvent::End, window, cx);
            }
        })
        .on_drag(
            DockResizeHandleDrag { area },
            |_: &DockResizeHandleDrag, _: Point<Pixels>, _: &mut Window, cx: &mut App| {
                cx.new(|_| EmptyView)
            },
        )
        .on_drag_move::<DockResizeHandleDrag>({
            let resize = resize.clone();
            move |event, window: &mut Window, cx: &mut App| {
                if event.drag(cx).area != area {
                    return;
                }
                resize(
                    DockResizeEvent::Drag {
                        position: event.event.position,
                    },
                    window,
                    cx,
                );
                cx.stop_propagation();
            }
        });

    handle = match area {
        DockAreaId::Left => handle
            .top_0()
            .right(HANDLE_OVERLAP)
            .h_full()
            .w(HANDLE_SIZE)
            .cursor_col_resize(),
        DockAreaId::Right => handle
            .top_0()
            .left(HANDLE_OVERLAP)
            .h_full()
            .w(HANDLE_SIZE)
            .cursor_col_resize(),
        DockAreaId::Bottom => handle
            .top(HANDLE_OVERLAP)
            .left_0()
            .w_full()
            .h(HANDLE_SIZE)
            .cursor_row_resize(),
    };

    // handle 用 `right/left/top` 负偏移横跨 dock 边界，会伸进相邻 region。
    // 相邻 region 若在本 dock 之后绘制（如 LeftDock 旁的 EditorArea），会盖住
    // 重叠条并截走 mouse_down，导致拖拽失效。deferred 把 handle 抬到顶层绘制，
    // 保证三个 dock 的 handle 都能稳定接收事件。
    deferred(handle).into_any_element()
}

fn handle_id(area: DockAreaId) -> &'static str {
    match area {
        DockAreaId::Left => "left-dock-resize-handle",
        DockAreaId::Right => "right-dock-resize-handle",
        DockAreaId::Bottom => "bottom-dock-resize-handle",
    }
}
