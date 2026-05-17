//! BottomBar —— 窗口级底部外壳（布局模型 4.3）。
//!
//! 第一版只用 leading / trailing 两个槽（无 center）。每个槽内按 Dock
//! 归属分组，组与组之间用一根 `bar_divider` 视觉隔开。
//!
//! ```text
//! Leading (贴左)  : [文件树 │ 版本管理 │ 大纲 │ 项目搜索]  ┃  [语言服务 │ 诊断]
//!                    ↑ 都打开在 LeftDock                       ↑ 不绑 Dock，纯状态
//! Trailing (贴右) :                       [终端 │ 调试]  ┃  [快捷键]
//!                                   ↑ 都打开在 BottomDock     ↑ 打开在 RightDock
//! ```
//!
//! 面板切换 slot 是 `panel.toggle.<id>` 命令的视图——BottomBar 不知道
//! panel 是什么，只 emit CommandId（骨架阶段尚未接入，先只显示状态）。

use gpui::{AnyElement, Div, IntoElement, div, prelude::*};

use crate::shell::layout::{DockAreaId, DockState, PanelId, WorkbenchState};

use crate::shell::components::primitives::{
    BarEdge, BarRegionAlign, Glyph, align_bar_region, bar_divider, bar_frame,
};

/// 一个 panel 切换槽：决定它打开 / 高亮哪一个 Dock 里的哪个 panel。
struct PanelSlot {
    id: &'static str,
    icon: &'static str,
    tooltip: &'static str,
    dock: DockAreaId,
    panel: PanelId,
}

/// Group 1（左槽第一段）：托管在 LeftDock 的四个 panel。
const LEADING_PANEL_GROUP: &[PanelSlot] = &[
    PanelSlot {
        id: "bottom-bar.file_tree",
        icon: "icons/bottom_bar/file_tree.svg",
        tooltip: "文件树",
        dock: DockAreaId::Left,
        panel: PanelId::FileTree,
    },
    PanelSlot {
        id: "bottom-bar.version_control",
        icon: "icons/bottom_bar/version_control.svg",
        tooltip: "版本管理",
        dock: DockAreaId::Left,
        panel: PanelId::VersionControl,
    },
    PanelSlot {
        id: "bottom-bar.outline",
        icon: "icons/bottom_bar/outline.svg",
        tooltip: "大纲",
        dock: DockAreaId::Left,
        panel: PanelId::Outline,
    },
    PanelSlot {
        id: "bottom-bar.project_search",
        icon: "icons/bottom_bar/project_search.svg",
        tooltip: "项目搜索",
        dock: DockAreaId::Left,
        panel: PanelId::ProjectSearch,
    },
];

/// Group 3（右槽第一段）：托管在 BottomDock 的两个 panel。
const TRAILING_PANEL_GROUP_BOTTOM: &[PanelSlot] = &[
    PanelSlot {
        id: "bottom-bar.terminal",
        icon: "icons/bottom_bar/terminal.svg",
        tooltip: "终端",
        dock: DockAreaId::Bottom,
        panel: PanelId::Terminal,
    },
    PanelSlot {
        id: "bottom-bar.debug",
        icon: "icons/bottom_bar/debug.svg",
        tooltip: "调试",
        dock: DockAreaId::Bottom,
        panel: PanelId::Debug,
    },
];

/// Group 4（右槽第二段）：托管在 RightDock 的 panel。
const TRAILING_PANEL_GROUP_RIGHT: &[PanelSlot] = &[PanelSlot {
    id: "bottom-bar.keyboard_shortcuts",
    icon: "icons/bottom_bar/keyboard_shortcuts.svg",
    tooltip: "快捷键",
    dock: DockAreaId::Right,
    panel: PanelId::KeyboardShortcuts,
}];

const DIAGNOSTICS_ID: &str = "bottom-bar.diagnostics";
const DIAGNOSTICS_ICON: &str = "icons/bottom_bar/diagnostics.svg";
const LSP_ID: &str = "bottom-bar.language_server";
const LSP_ICON: &str = "icons/bottom_bar/language_server.svg";

pub(crate) fn render(state: &WorkbenchState) -> Div {
    bar_frame(BarEdge::Bottom)
        .child(region(leading_slots(state), BarRegionAlign::Leading))
        .child(region(trailing_slots(state), BarRegionAlign::Trailing))
}

fn region(items: Vec<AnyElement>, align: BarRegionAlign) -> Div {
    // inner 必须内容自适应；外层 `align_bar_region` 已经 flex_1 + justify_*。
    let inner = div().flex().items_center().gap_2().children(items);
    align_bar_region(inner, align)
}

fn leading_slots(state: &WorkbenchState) -> Vec<AnyElement> {
    let toggles = panel_slot_group(LEADING_PANEL_GROUP, state);
    // Group 2：语言服务 / 诊断。第一版暂不绑 Dock；纯状态指示，无 active 高亮语义。
    let status = vec![
        lsp_slot(state.bottom_bar.lsp_connected),
        diagnostics_slot(state.bottom_bar.diagnostics_count),
    ];
    join_groups(vec![toggles, status])
}

fn trailing_slots(state: &WorkbenchState) -> Vec<AnyElement> {
    let bottom = panel_slot_group(TRAILING_PANEL_GROUP_BOTTOM, state);
    let right = panel_slot_group(TRAILING_PANEL_GROUP_RIGHT, state);
    join_groups(vec![bottom, right])
}

/// 把多个组拼起来，组与组之间插入一条 `bar_divider`。空组直接跳过。
fn join_groups(groups: Vec<Vec<AnyElement>>) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();
    for group in groups.into_iter().filter(|g| !g.is_empty()) {
        if !out.is_empty() {
            out.push(bar_divider().into_any_element());
        }
        out.extend(group);
    }
    out
}

fn panel_slot_group(slots: &[PanelSlot], state: &WorkbenchState) -> Vec<AnyElement> {
    slots
        .iter()
        .map(|slot| panel_slot(slot, dock_state_for(slot.dock, state)))
        .collect()
}

fn dock_state_for(area: DockAreaId, state: &WorkbenchState) -> &DockState {
    match area {
        DockAreaId::Left => &state.left_dock,
        DockAreaId::Right => &state.right_dock,
        DockAreaId::Bottom => &state.bottom_dock,
    }
}

/// 高亮规则：本槽对应的 panel 在它所属 Dock 中正激活且 Dock 可见。
fn panel_slot(slot: &PanelSlot, dock_state: &DockState) -> AnyElement {
    let active = dock_state.is_visible() && dock_state.active_panel() == Some(slot.panel);

    Glyph::icon(slot.id, slot.icon, slot.tooltip)
        .active(active)
        .render()
}

fn lsp_slot(connected: bool) -> AnyElement {
    Glyph::icon(LSP_ID, LSP_ICON, "语言服务")
        .active(connected)
        .render()
}

fn diagnostics_slot(count: u32) -> AnyElement {
    Glyph::icon_text(DIAGNOSTICS_ID, DIAGNOSTICS_ICON, count.to_string(), "诊断").render()
}
