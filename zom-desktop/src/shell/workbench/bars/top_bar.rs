//! TopBar —— 窗口级顶部外壳（布局模型 4.2）。
//!
//! 当前固定槽：
//! - leading：窗口控制圆点 + 项目入口
//! - trailing：设置入口
//!
//! 与 BottomBar 共用 `bar_frame`，确保对称（布局模型 4.1）。

use gpui::{AnyElement, Div, Window, div, prelude::*};

use crate::shell::features::{project_picker, settings};
use crate::shell::workbench::state::WorkbenchState;
use crate::shell::{CommandTitleLookup, ShortcutLookup};

use super::frame::{BarEdge, BarRegionAlign, align_bar_region, bar_frame};
use super::window_controls::{WindowControlsHandlers, render_window_controls};

pub(crate) fn render(
    state: &WorkbenchState,
    window: &Window,
    window_controls: WindowControlsHandlers,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
    workspace_active: bool,
) -> Div {
    let is_window_active = window.is_window_active();

    bar_frame(BarEdge::Top)
        .child(region(
            leading_slots(
                is_window_active,
                window_controls,
                shortcuts,
                titles,
                workspace_active,
                &state.project_title,
            ),
            BarRegionAlign::Leading,
        ))
        .child(region(Vec::new(), BarRegionAlign::Center))
        .child(region(
            trailing_slots(shortcuts, titles),
            BarRegionAlign::Trailing,
        ))
}

fn region(items: Vec<AnyElement>, align: BarRegionAlign) -> Div {
    // inner 必须内容自适应；外层 `align_bar_region` 已经 flex_1 + justify_*。
    // 如果 inner 也写 flex_1，会撑满外层，justify_end / center 失效。
    let inner = div().flex().items_center().gap_2().children(items);
    align_bar_region(inner, align)
}

fn leading_slots(
    is_window_active: bool,
    window_controls: WindowControlsHandlers,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
    workspace_active: bool,
    project_title: &str,
) -> Vec<AnyElement> {
    let workspace = project_picker::entry(project_title, workspace_active, shortcuts, titles);

    vec![
        render_window_controls(is_window_active, window_controls).into_any_element(),
        workspace,
    ]
}

fn trailing_slots(shortcuts: &ShortcutLookup, titles: &CommandTitleLookup) -> Vec<AnyElement> {
    vec![settings::entry(shortcuts, titles)]
}
