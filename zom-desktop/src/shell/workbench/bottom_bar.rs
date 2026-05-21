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
use crate::shell::workbench::docks::{bottom, left, right};
use crate::shell::workbench::element_ids;
use crate::shell::workbench::overlays::{AnchorRegistry, track_anchor};
use crate::shell::workbench::state::{DockAreaId, DockState, EditorState, WorkbenchState};

use super::bars::{BarEdge, BarRegionAlign, Glyph, align_bar_region, bar_divider, bar_frame};

const DIAGNOSTICS_ID: &str = "bottom-bar.diagnostics";
const DIAGNOSTICS_ICON: &str = "icons/bottom_bar/diagnostics.svg";
const DIAGNOSTICS_COMMAND: &str = diagnostics::SHOW_PROBLEMS;
const LSP_ICON: &str = "icons/bottom_bar/language_server.svg";
const LSP_COMMAND: &str = language_server_commands::OPEN_STATUS;
const CURSOR_POSITION_ID: &str = "bottom-bar.cursor-position";
const LANGUAGE_ID: &str = "bottom-bar.language";

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
    let toggles = panel_slot_group(DockAreaId::Left, left::PANELS, state, shortcuts);
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
    let editor = editor_status_slots(&state.editor, shortcuts);
    let bottom = panel_slot_group(DockAreaId::Bottom, bottom::PANELS, state, shortcuts);
    let right = panel_slot_group(DockAreaId::Right, right::PANELS, state, shortcuts);
    join_groups(vec![editor, bottom, right])
}

/// 活动文件状态组：光标行列 + 语言类型。没有打开文件时返回空组。
fn editor_status_slots(editor: &EditorState, shortcuts: &ShortcutLookup) -> Vec<AnyElement> {
    let Some(active) = editor.tabs.iter().find(|tab| tab.is_active) else {
        return Vec::new();
    };
    let (line, column) = cursor_line_column(&editor.text, editor.cursor_byte);
    vec![
        Glyph::text(
            CURSOR_POSITION_ID,
            format!("{line}:{column}"),
            "光标位置（行:列）",
        )
        .render(shortcuts),
        Glyph::text(LANGUAGE_ID, language_label(&active.title), "文件语言").render(shortcuts),
    ]
}

/// 把字节光标位置换算成 1 基的行 / 列（列按字符计）。
fn cursor_line_column(text: &str, cursor_byte: usize) -> (usize, usize) {
    let cursor_byte = cursor_byte.min(text.len());
    let before = text.get(..cursor_byte).unwrap_or("");
    let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let column = before[line_start..].chars().count() + 1;
    (line, column)
}

/// 由文件名后缀推断语言显示名；未知后缀回退为大写后缀，无后缀为「纯文本」。
fn language_label(title: &str) -> String {
    match std::path::Path::new(title)
        .extension()
        .and_then(|ext| ext.to_str())
    {
        Some("rs") => "Rust".to_string(),
        Some("toml") | Some("lock") => "TOML".to_string(),
        Some("md") | Some("markdown") => "Markdown".to_string(),
        Some("json") => "JSON".to_string(),
        Some("js") | Some("mjs") | Some("cjs") => "JavaScript".to_string(),
        Some("ts") => "TypeScript".to_string(),
        Some("jsx") => "JSX".to_string(),
        Some("tsx") => "TSX".to_string(),
        Some("html") | Some("htm") => "HTML".to_string(),
        Some("css") => "CSS".to_string(),
        Some("scss") | Some("sass") => "Sass".to_string(),
        Some("yaml") | Some("yml") => "YAML".to_string(),
        Some("xml") => "XML".to_string(),
        Some("py") => "Python".to_string(),
        Some("go") => "Go".to_string(),
        Some("c") | Some("h") => "C".to_string(),
        Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => "C++".to_string(),
        Some("java") => "Java".to_string(),
        Some("kt") | Some("kts") => "Kotlin".to_string(),
        Some("swift") => "Swift".to_string(),
        Some("rb") => "Ruby".to_string(),
        Some("php") => "PHP".to_string(),
        Some("sh") | Some("bash") | Some("zsh") => "Shell".to_string(),
        Some("sql") => "SQL".to_string(),
        Some("ini") | Some("conf") | Some("cfg") => "INI".to_string(),
        Some("txt") | Some("text") => "Text".to_string(),
        Some("csv") => "CSV".to_string(),
        Some("svg") => "SVG".to_string(),
        Some(other) => other.to_uppercase(),
        None => "Unknown".to_string(),
    }
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
