use std::time::Duration;

use gpui::{Bounds, Pixels, Point, point};

/// 拖拽选区自动滚动的统一节拍（≈60Hz）。
///
/// 自动滚动与输入事件频率解耦：拖动事件只更新意图，由持有者按此节拍调度。
/// Editor 与 Terminal 的选择手势共用同一节拍与越界策略。
pub const AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(16);

/// 拖拽选区自动滚动的统一状态。
///
/// 只表达与设备无关的「指针位置 + 当前越界滚动量」；
/// 16ms 定时器由持有它的领域实体按自身生命周期调度（Editor 的选择手势、Terminal 的选择手势各自拥有），本类型不持有任务，避免成为第二份可写的选择或滚动权威。
#[derive(Clone, Copy, Debug)]
pub struct DragAutoscroll {
    position: Point<Pixels>,
    delta: Point<Pixels>,
}

impl Default for DragAutoscroll {
    fn default() -> Self {
        Self {
            position: point(Pixels::ZERO, Pixels::ZERO),
            delta: point(Pixels::ZERO, Pixels::ZERO),
        }
    }
}

impl DragAutoscroll {
    /// 用最新指针位置刷新越界滚动量。
    pub fn update(&mut self, position: Point<Pixels>, bounds: Bounds<Pixels>, line_height: Pixels) {
        self.position = position;
        self.delta = selection_autoscroll_delta(position, bounds, line_height);
    }

    pub fn position(&self) -> Point<Pixels> {
        self.position
    }

    pub fn delta(&self) -> Point<Pixels> {
        self.delta
    }

    /// 指针是否越出了正文视口、当前是否需要继续滚动。
    pub fn is_active(&self) -> bool {
        self.delta != point(Pixels::ZERO, Pixels::ZERO)
    }

    pub fn clear(&mut self) {
        self.position = point(Pixels::ZERO, Pixels::ZERO);
        self.delta = point(Pixels::ZERO, Pixels::ZERO);
    }
}

/// 拖拽选区的自动滚动量：仅在指针越出正文视口后产生。
///
/// 竖直方向保留 `min(行高, 视口高/3)` 的死区，水平方向无死区；
/// 越界距离按线性系数放大，单次增量按视口尺寸封顶，避免快速甩动猛跳。
pub fn selection_autoscroll_delta(
    position: Point<Pixels>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
) -> Point<Pixels> {
    let edge_margin = point(Pixels::ZERO, line_height.min(bounds.size.height / 3.0));
    drag_autoscroll_delta(
        position,
        bounds,
        edge_margin,
        point(bounds.size.width / 16.0, bounds.size.height / 16.0),
    )
}

/// 根据指针距视口边缘的距离计算拖拽自动滚动量。
///
/// 返回值只描述位移，不修改滚动状态。
/// 正的 `y` 表示内容向上回看，负的 `y`表示继续查看下方内容；
/// 正的 `x` 表示内容向左回看，负的 `x` 表示继续查看右侧内容；
/// 调用方负责按照自身滚动模型应用并钳制结果。
pub fn drag_autoscroll_delta(
    position: Point<Pixels>,
    bounds: Bounds<Pixels>,
    edge_margin: Point<Pixels>,
    max_delta: Point<Pixels>,
) -> Point<Pixels> {
    let mut delta = point(Pixels::ZERO, Pixels::ZERO);
    let speed = 0.3;

    let top = bounds.origin.y + edge_margin.y;
    let bottom = bounds.bottom_left().y - edge_margin.y;
    if position.y < top {
        delta.y = ((top - position.y) * speed).min(max_delta.y);
    } else if position.y > bottom {
        delta.y = -((position.y - bottom) * speed).min(max_delta.y);
    }

    let left = bounds.origin.x + edge_margin.x;
    let right = bounds.top_right().x - edge_margin.x;
    if position.x < left {
        delta.x = ((left - position.x) * speed).min(max_delta.x);
    } else if position.x > right {
        delta.x = -((position.x - right) * speed).min(max_delta.x);
    }

    delta
}

#[cfg(test)]
#[path = "test/autoscroll_tests.rs"]
mod tests;
