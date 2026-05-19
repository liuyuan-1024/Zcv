//! BottomBar —— 窗口级底部外壳（布局模型 4.3）。
//!
//! 第一版只用 leading / trailing 两个槽（无 center）。每个槽内按 Dock
//! 归属分组，组与组之间用一根 `bar_divider` 视觉隔开。
//!
//! 面板切换 slot 是 `panel.toggle.<id>` 命令的视图——BottomBar 不知道
//! panel 是什么，只 emit CommandId（骨架阶段尚未接入，先只显示状态）。

use gpui::{AnyElement, Div, Entity, IntoElement, div, prelude::*};

use zom_command::commands::{diagnostics, language_server as language_server_commands};

use crate::shell::ShortcutLookup;
use crate::shell::features::PanelId;
use crate::shell::shared::element_ids;
use crate::shell::shared::primitives::{
    BarEdge, BarRegionAlign, Glyph, align_bar_region, bar_divider, bar_frame,
};
use crate::shell::workbench::overlay::{AnchorRegistry, track_anchor};
use crate::shell::workbench::state::{DockAreaId, DockState, WorkbenchState};

use super::{bottom_dock, left_dock, right_dock};

const DIAGNOSTICS_ID: &str = "bottom-bar.diagnostics";
const DIAGNOSTICS_ICON: &str = "icons/bottom_bar/diagnostics.svg";
const DIAGNOSTICS_COMMAND: &str = diagnostics::SHOW_PROBLEMS;
const LSP_ICON: &str = "icons/bottom_bar/language_server.svg";
const LSP_COMMAND: &str = language_server_commands::OPEN_STATUS;

pub(crate) fn render(
    state: &WorkbenchState,
    shortcuts: &ShortcutLookup,
    anchor_registry: Entity<AnchorRegistry>,
    language_server_active: bool,
) -> Div {
    bar_frame(BarEdge::Bottom)
        .child(region(
            leading_slots(state, shortcuts, anchor_registry, language_server_active),
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

fn leading_slots(
    state: &WorkbenchState,
    shortcuts: &ShortcutLookup,
    anchor_registry: Entity<AnchorRegistry>,
    language_server_active: bool,
) -> Vec<AnyElement> {
    let toggles = panel_slot_group(DockAreaId::Left, left_dock::PANELS, state, shortcuts);
    // Group 2：语言服务器 / 诊断。第一版暂不绑 Dock；纯状态指示，但仍可关联命令
    // 入口（"打开 LSP 状态" / "查看问题面板"）。
    let status = vec![
        lsp_slot(
            state.bottom_bar.lsp_connected,
            language_server_active,
            shortcuts,
            anchor_registry,
        ),
        diagnostics_slot(state.bottom_bar.diagnostics_count, shortcuts),
    ];
    join_groups(vec![toggles, status])
}

fn trailing_slots(state: &WorkbenchState, shortcuts: &ShortcutLookup) -> Vec<AnyElement> {
    let bottom = panel_slot_group(DockAreaId::Bottom, bottom_dock::PANELS, state, shortcuts);
    let right = panel_slot_group(DockAreaId::Right, right_dock::PANELS, state, shortcuts);
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
    area: DockAreaId,
    panels: &[PanelId],
    state: &WorkbenchState,
    shortcuts: &ShortcutLookup,
) -> Vec<AnyElement> {
    panels
        .iter()
        .copied()
        .map(|panel| panel_slot(panel, dock_state_for(area, state), shortcuts))
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
fn panel_slot(panel: PanelId, dock_state: &DockState, shortcuts: &ShortcutLookup) -> AnyElement {
    let active = dock_state.is_visible() && dock_state.active_panel() == Some(panel);

    Glyph::icon(
        panel_glyph_id(panel),
        panel.icon_path(),
        panel_tooltip(panel),
    )
    .command(panel.toggle_command_id())
    .active(active)
    .render(shortcuts)
}

/// bottom bar 内 panel 入口 glyph 的 element id —— GPUI 用它跟踪 element
/// 身份，与命令 id 无关；从 PanelId 派生避免散落字符串。
fn panel_glyph_id(panel: PanelId) -> gpui::SharedString {
    format!("bottom-bar.{}", panel.command_str_id()).into()
}

/// Panel 图标的 tooltip 文案。只有 BottomBar 用，所以集中在这里、不外抽。
fn panel_tooltip(panel: PanelId) -> &'static str {
    match panel {
        PanelId::FileTree => "文件树",
        PanelId::VersionControl => "版本管理",
        PanelId::Outline => "大纲",
        PanelId::ProjectSearch => "项目搜索",
        PanelId::Terminal => "终端",
        PanelId::Debug => "调试",
        PanelId::KeyboardShortcuts => "快捷键",
    }
}

fn lsp_slot(
    connected: bool,
    overlay_active: bool,
    shortcuts: &ShortcutLookup,
    anchor_registry: Entity<AnchorRegistry>,
) -> AnyElement {
    let glyph = Glyph::icon(
        element_ids::BOTTOM_BAR_LANGUAGE_SERVER,
        LSP_ICON,
        "语言服务器",
    )
    .command(LSP_COMMAND)
    .active(connected || overlay_active)
    .render(shortcuts);

    track_anchor(
        element_ids::BOTTOM_BAR_LANGUAGE_SERVER,
        anchor_registry,
        glyph,
    )
    .into_any_element()
}

fn diagnostics_slot(count: u32, shortcuts: &ShortcutLookup) -> AnyElement {
    Glyph::icon_text(DIAGNOSTICS_ID, DIAGNOSTICS_ICON, count.to_string(), "诊断")
        .command(DIAGNOSTICS_COMMAND)
        .render(shortcuts)
}
