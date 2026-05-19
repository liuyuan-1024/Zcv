//! TopBar —— 窗口级顶部外壳（布局模型 4.2）。
//!
//! 第一版固定槽：
//! - leading：窗口控制圆点 + 项目入口
//! - center：（暂空，将来承载命令面板入口或运行中任务摘要）
//! - trailing：设置入口
//!
//! 与 BottomBar 共用 `bar_frame`，确保对称（布局模型 4.1）。

use gpui::{AnyElement, Div, Entity, Window, div, prelude::*};

use crate::shell::shared::element_ids;
use crate::shell::shared::primitives::{
    BarEdge, BarRegionAlign, Glyph, align_bar_region, bar_frame,
};
use crate::shell::workbench::overlay::{AnchorRegistry, track_anchor};
use crate::shell::workbench::state::WorkbenchState;
use crate::shell::{ShortcutLookup, WindowControlsHandlers};

mod window_controls;
use window_controls::render_window_controls;

use zom_command::commands::{settings, workspace as workspace_commands};

const WORKSPACE_TOOLTIP: &str = "切换项目";
const WORKSPACE_COMMAND: &str = workspace_commands::SHOW_PROJECTS_PICKER;

const SETTINGS_ID: &str = "top-bar.settings";
const SETTINGS_ICON: &str = "icons/top_bar/settings.svg";
const SETTINGS_TOOLTIP: &str = "设置";
const SETTINGS_COMMAND: &str = settings::OPEN;

pub(crate) fn render(
    state: &WorkbenchState,
    window: &Window,
    window_controls: WindowControlsHandlers,
    shortcuts: &ShortcutLookup,
    anchor_registry: Entity<AnchorRegistry>,
    workspace_active: bool,
) -> Div {
    let is_window_active = window.is_window_active();

    bar_frame(BarEdge::Top)
        .child(region(
            leading_slots(
                is_window_active,
                window_controls,
                shortcuts,
                anchor_registry,
                workspace_active,
                &state.project_title,
            ),
            BarRegionAlign::Leading,
        ))
        .child(region(Vec::new(), BarRegionAlign::Center))
        .child(region(trailing_slots(shortcuts), BarRegionAlign::Trailing))
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
    anchor_registry: Entity<AnchorRegistry>,
    workspace_active: bool,
    project_title: &str,
) -> Vec<AnyElement> {
    let workspace = Glyph::text(
        element_ids::TOP_BAR_WORKSPACE,
        project_title,
        WORKSPACE_TOOLTIP,
    )
    .command(WORKSPACE_COMMAND)
    .active(workspace_active)
    .render(shortcuts);

    vec![
        render_window_controls(is_window_active, window_controls).into_any_element(),
        track_anchor(element_ids::TOP_BAR_WORKSPACE, anchor_registry, workspace).into_any_element(),
    ]
}

fn trailing_slots(shortcuts: &ShortcutLookup) -> Vec<AnyElement> {
    vec![
        Glyph::icon(SETTINGS_ID, SETTINGS_ICON, SETTINGS_TOOLTIP)
            .command(SETTINGS_COMMAND)
            .render(shortcuts),
    ]
}
