use gpui::{Bounds, Pixels, Point, point};

/// 根据指针距视口边缘的距离计算拖拽自动滚动量。
///
/// 返回值只描述位移，不修改滚动状态。
/// 正的 `y` 表示内容向上回看，负的 `y`表示继续查看下方内容；
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
        delta.x = -((left - position.x) * speed).min(max_delta.x);
    } else if position.x > right {
        delta.x = ((position.x - right) * speed).min(max_delta.x);
    }

    delta
}

#[cfg(test)]
#[path = "test/autoscroll_tests.rs"]
mod tests;
