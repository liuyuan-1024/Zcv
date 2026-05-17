//! TopBar —— 窗口级顶部外壳（布局模型 4.2）。
//!
//! 第一版固定槽：
//! - leading：窗口控制圆点 + workspace 入口
//! - center：（暂空，将来承载命令面板入口或运行中任务摘要）
//! - trailing：设置入口
//!
//! 与 BottomBar 共用 `bar_frame`，确保对称（布局模型 4.1）。

use gpui::{AnyElement, Div, Window, div, prelude::*};

use crate::shell::components::primitives::{
    BarEdge, BarRegionAlign, Glyph, align_bar_region, bar_frame,
};

mod window_controls;
use window_controls::render_window_controls;

const WORKSPACE_LABEL_ID: &str = "top-bar.workspace";
const WORKSPACE_LABEL: &str = "zom";
const WORKSPACE_TOOLTIP: &str = "工作区";

const SETTINGS_ID: &str = "top-bar.settings";
const SETTINGS_ICON: &str = "icons/top_bar/settings.svg";
const SETTINGS_TOOLTIP: &str = "设置";

pub(crate) fn render(window: &Window) -> Div {
    let is_window_active = window.is_window_active();

    bar_frame(BarEdge::Top)
        .child(region(
            leading_slots(is_window_active),
            BarRegionAlign::Leading,
        ))
        .child(region(Vec::new(), BarRegionAlign::Center))
        .child(region(trailing_slots(), BarRegionAlign::Trailing))
}

fn region(items: Vec<AnyElement>, align: BarRegionAlign) -> Div {
    // inner 必须内容自适应；外层 `align_bar_region` 已经 flex_1 + justify_*。
    // 如果 inner 也写 flex_1，会撑满外层，justify_end / center 失效。
    let inner = div().flex().items_center().gap_2().children(items);
    align_bar_region(inner, align)
}

fn leading_slots(is_window_active: bool) -> Vec<AnyElement> {
    vec![
        render_window_controls(is_window_active).into_any_element(),
        Glyph::text(WORKSPACE_LABEL_ID, WORKSPACE_LABEL, WORKSPACE_TOOLTIP).render(),
    ]
}

fn trailing_slots() -> Vec<AnyElement> {
    vec![Glyph::icon(SETTINGS_ID, SETTINGS_ICON, SETTINGS_TOOLTIP).render()]
}
