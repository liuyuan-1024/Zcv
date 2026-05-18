//! BottomBar —— 窗口级底部外壳（布局模型 4.3）。
//!
//! 第一版只用 leading / trailing 两个槽（无 center）。每个槽内按 Dock
//! 归属分组，组与组之间用一根 `bar_divider` 视觉隔开。
//!
//! 面板切换 slot 是 `panel.toggle.<id>` 命令的视图——BottomBar 不知道
//! panel 是什么，只 emit CommandId（骨架阶段尚未接入，先只显示状态）。

use gpui::{AnyElement, Div, IntoElement, div, prelude::*};

use zom_command::commands::diagnostics;

use crate::shell::ShortcutLookup;
use crate::shell::model::{DockAreaId, DockState, PanelId, WorkbenchState};
use crate::shell::primitives::{
    BarEdge, BarRegionAlign, Glyph, align_bar_region, bar_divider, bar_frame,
};

/// 一个 panel 切换槽：仅声明"这个 panel 装在哪个 Dock"。
///
/// 图标 / 标题 / 命令 id / glyph element id 全部从 [`PanelId`] 派生 ——
/// PanelId 是 panel 元数据的单一真理源，槽表只补它推不出的"位置"。
struct PanelSlot {
    dock: DockAreaId,
    panel: PanelId,
}

const LEADING_PANEL_GROUP: &[PanelSlot] = &[
    PanelSlot {
        dock: DockAreaId::Left,
        panel: PanelId::FileTree,
    },
    PanelSlot {
        dock: DockAreaId::Left,
        panel: PanelId::VersionControl,
    },
    PanelSlot {
        dock: DockAreaId::Left,
        panel: PanelId::Outline,
    },
    PanelSlot {
        dock: DockAreaId::Left,
        panel: PanelId::ProjectSearch,
    },
];

const TRAILING_PANEL_GROUP_BOTTOM: &[PanelSlot] = &[
    PanelSlot {
        dock: DockAreaId::Bottom,
        panel: PanelId::Terminal,
    },
    PanelSlot {
        dock: DockAreaId::Bottom,
        panel: PanelId::Debug,
    },
];

const TRAILING_PANEL_GROUP_RIGHT: &[PanelSlot] = &[PanelSlot {
    dock: DockAreaId::Right,
    panel: PanelId::KeyboardShortcuts,
}];

const DIAGNOSTICS_ID: &str = "bottom-bar.diagnostics";
const DIAGNOSTICS_ICON: &str = "icons/bottom_bar/diagnostics.svg";
const DIAGNOSTICS_COMMAND: &str = diagnostics::SHOW_PROBLEMS;
const LSP_ID: &str = "bottom-bar.language_server";
const LSP_ICON: &str = "icons/bottom_bar/language_server.svg";
const LSP_COMMAND: &str = diagnostics::OPEN_LSP_STATUS;

pub(crate) fn render(state: &WorkbenchState, shortcuts: &ShortcutLookup) -> Div {
    bar_frame(BarEdge::Bottom)
        .child(region(
            leading_slots(state, shortcuts),
            BarRegionAlign::Leading,
        ))
        .child(region(
            trailing_slots(state, shortcuts),
            BarRegionAlign::Trailing,
        ))
}

fn region(items: Vec<AnyElement>, align: BarRegionAlign) -> Div {
    // inner 必须内容自适应；外层 `align_bar_region` 已经 flex_1 + justify_*。
    let inner = div().flex().items_center().gap_2().children(items);
    align_bar_region(inner, align)
}

fn leading_slots(state: &WorkbenchState, shortcuts: &ShortcutLookup) -> Vec<AnyElement> {
    let toggles = panel_slot_group(LEADING_PANEL_GROUP, state, shortcuts);
    // Group 2：语言服务 / 诊断。第一版暂不绑 Dock；纯状态指示，但仍可关联命令
    // 入口（"打开 LSP 状态" / "查看问题面板"）。
    let status = vec![
        lsp_slot(state.bottom_bar.lsp_connected, shortcuts),
        diagnostics_slot(state.bottom_bar.diagnostics_count, shortcuts),
    ];
    join_groups(vec![toggles, status])
}

fn trailing_slots(state: &WorkbenchState, shortcuts: &ShortcutLookup) -> Vec<AnyElement> {
    let bottom = panel_slot_group(TRAILING_PANEL_GROUP_BOTTOM, state, shortcuts);
    let right = panel_slot_group(TRAILING_PANEL_GROUP_RIGHT, state, shortcuts);
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

fn panel_slot_group(
    slots: &[PanelSlot],
    state: &WorkbenchState,
    shortcuts: &ShortcutLookup,
) -> Vec<AnyElement> {
    slots
        .iter()
        .map(|slot| panel_slot(slot, dock_state_for(slot.dock, state), shortcuts))
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
fn panel_slot(slot: &PanelSlot, dock_state: &DockState, shortcuts: &ShortcutLookup) -> AnyElement {
    let panel = slot.panel;
    let active = dock_state.is_visible() && dock_state.active_panel() == Some(panel);

    Glyph::icon(panel_glyph_id(panel), panel.icon_path(), panel.title())
        .command(panel.toggle_command_id())
        .active(active)
        .render(shortcuts)
}

/// bottom bar 内 panel 入口 glyph 的 element id —— GPUI 用它跟踪 element
/// 身份，与命令 id 无关；从 PanelId 派生避免散落字符串。
fn panel_glyph_id(panel: PanelId) -> gpui::SharedString {
    format!("bottom-bar.{}", panel.command_str_id()).into()
}

fn lsp_slot(connected: bool, shortcuts: &ShortcutLookup) -> AnyElement {
    Glyph::icon(LSP_ID, LSP_ICON, "语言服务")
        .command(LSP_COMMAND)
        .active(connected)
        .render(shortcuts)
}

fn diagnostics_slot(count: u32, shortcuts: &ShortcutLookup) -> AnyElement {
    Glyph::icon_text(DIAGNOSTICS_ID, DIAGNOSTICS_ICON, count.to_string(), "诊断")
        .command(DIAGNOSTICS_COMMAND)
        .render(shortcuts)
}
