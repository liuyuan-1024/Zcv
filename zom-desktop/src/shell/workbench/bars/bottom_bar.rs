//! BottomBar —— 窗口级底部外壳（布局模型 4.3）。
//!
//! 当前只用 leading / trailing 两个槽（无 center）。每个槽内按 Dock
//! 归属分组，组与组之间用一根 `bar_divider` 视觉隔开。
//!
//! 面板切换 slot 是 `panel.toggle.<id>` 命令的视图——BottomBar 不知道
//! panel 是什么，只 emit CommandId。

use gpui::{AnyElement, Div, IntoElement, div, prelude::*};

use crate::editor::text::EditorSnapshot;
use crate::editor_state::EditorState;
use crate::shell::features::{diagnostics, language_servers};
use crate::shell::shared::Glyph;
use crate::shell::workbench::docks::{bottom, left, right};
use crate::shell::workbench::state::{DockAreaId, DockState, WorkbenchState};
use crate::shell::{CommandTitleLookup, ShortcutLookup};
use crate::ui_id::{self, PanelId};
use zom_command::commands::search;

use super::frame::{BarEdge, BarRegionAlign, align_bar_region, bar_divider, bar_frame};

const CURSOR_POSITION_ID: &str = "bottom-bar.cursor-position";
const LANGUAGE_ID: &str = "bottom-bar.language";

pub(crate) fn render(
    state: &WorkbenchState,
    editor: &EditorState,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
    language_server_active: bool,
    main_editor_snapshot: &EditorSnapshot,
) -> Div {
    bar_frame(BarEdge::Bottom)
        .child(region(
            leading_slots(state, shortcuts, titles, language_server_active),
            BarRegionAlign::Leading,
        ))
        .child(region(
            trailing_slots(state, editor, shortcuts, titles, main_editor_snapshot),
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
    titles: &CommandTitleLookup,
    language_server_active: bool,
) -> Vec<AnyElement> {
    let toggles = panel_slot_group(DockAreaId::Left, left::PANELS, state, shortcuts, titles);
    // Group 2：语言服务器 / 诊断 / 项目级搜索。当前都不绑 Dock；混了状态指示与命令入口。
    let status = vec![
        language_servers::entry(
            state.bottom_bar.lsp_connected,
            language_server_active,
            shortcuts,
            titles,
        ),
        diagnostics::entry(state.bottom_bar.diagnostics_count, shortcuts, titles),
        project_search_slot(shortcuts, titles),
    ];
    join_groups(vec![toggles, status])
}

fn project_search_slot(shortcuts: &ShortcutLookup, titles: &CommandTitleLookup) -> AnyElement {
    let command_id = search::PROJECT_ACTIVATE;
    let title = titles(command_id).unwrap_or_else(|| command_id.to_string());
    Glyph::icon(
        "bottom-bar.search.project",
        "icons/panels/search.svg",
        title,
    )
    .hint(shortcuts(command_id))
    .active(false)
    .render()
}

fn trailing_slots(
    state: &WorkbenchState,
    editor_state: &EditorState,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
    main_editor_snapshot: &EditorSnapshot,
) -> Vec<AnyElement> {
    let editor = editor_status_slots(editor_state, main_editor_snapshot);
    let bottom = panel_slot_group(DockAreaId::Bottom, bottom::PANELS, state, shortcuts, titles);
    let right = panel_slot_group(DockAreaId::Right, right::PANELS, state, shortcuts, titles);
    join_groups(vec![editor, bottom, right])
}

/// 活动文件状态组：光标行列 + 语言类型。没有打开文件时返回空组。
fn editor_status_slots(editor: &EditorState, snapshot: &EditorSnapshot) -> Vec<AnyElement> {
    let Some(active) = editor.tabs.iter().find(|tab| tab.is_active) else {
        return Vec::new();
    };
    // snapshot 已经按 `buffer.byte_to_position` 折好行列（0-based）；这里只换算到 1-based。
    // 不再扫描 text，长文档也是 O(1)。
    let (line0, column0) = snapshot.cursor_position;
    let line = line0 + 1;
    let column = column0 + 1;
    vec![
        Glyph::text(
            CURSOR_POSITION_ID,
            format!("{line}:{column}"),
            "光标位置（行:列）",
        )
        .render(),
        Glyph::text(LANGUAGE_ID, active.language.clone(), "文件语言").render(),
    ]
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
    titles: &CommandTitleLookup,
) -> Vec<AnyElement> {
    panels
        .iter()
        .copied()
        .map(|panel| panel_slot(panel, dock_state_for(area, state), shortcuts, titles))
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
fn panel_slot(
    panel: PanelId,
    dock_state: &DockState,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
) -> AnyElement {
    let active = dock_state.is_visible() && dock_state.active_panel() == Some(panel);
    let command_id = panel.toggle_command_id();
    let title = titles(command_id).unwrap_or_else(|| command_id.to_string());

    Glyph::icon(panel_glyph_id(panel), ui_id::panel_icon_path(panel), title)
        .hint(shortcuts(command_id))
        .active(active)
        .render()
}

/// bottom bar 内 panel 入口 glyph 的 element id —— GPUI 用它跟踪 element
/// 身份，与命令 id 无关；从 PanelId 派生避免散落字符串。
fn panel_glyph_id(panel: PanelId) -> gpui::SharedString {
    format!("bottom-bar.{}", panel.slug()).into()
}
