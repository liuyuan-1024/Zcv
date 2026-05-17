//! OverlayLayer —— 临时可交互悬浮层 portal（布局模型 7 / 手册 21）。
//!
//! 第一版骨架：保留 portal 容器与 z-index 槽，但 `OverlayManager` 暂未引入；
//! 无 active overlay 时本层不渲染任何内容（也不拦截事件）。

use gpui::{Div, div, prelude::*};

pub(crate) fn render() -> Div {
    div().absolute().top_0().left_0().size_full().invisible()
}
