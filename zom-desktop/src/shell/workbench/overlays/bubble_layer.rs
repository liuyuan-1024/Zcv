//! BubbleLayer —— 轻量气泡提示层 portal（布局模型 8 / 手册 21）。
//!
//! 第一版骨架：保留 portal 容器与 z-index 槽，但 `BubbleManager` 暂未引入；
//! 无 active bubble 时本层不渲染任何内容。

use gpui::{Div, div, prelude::*};

pub(crate) fn render() -> Div {
    div().absolute().top_0().left_0().size_full().invisible()
}
