//! `panel.toggle.<panel_id>` 命令目录。
//!
//! handler emit [`HostEffect::TogglePanel`]，宿主负责把字符串 panel id 解析
//! 成它自己的枚举（避免 zom-command import 宿主类型）。
//!
//! 命令 id 与 panel 字符串 id 一一对应：`panel.toggle.<panel_str_id>`。
//! 加新 panel 在 [`PANELS`] 表里追加一行即可，handler 通用。

use crate::{
    CommandArgs, CommandId, CommandOutcome, CommandRegistry, HostEffect, Invocation, Keymap, NoArgs,
};

/// 一条 panel 切换命令的全部声明。
///
/// `panel_str_id` 既是宿主侧 PanelId 的字符串形式，也是命令 id 的后缀 ——
/// 拼接出 `panel.toggle.<panel_str_id>` 作为命令 id。
struct PanelEntry {
    panel_str_id: &'static str,
    command_id: &'static str,
    title: &'static str,
    default_chord: &'static str,
}

/// 全部 panel 切换的声明表。**改键位 / 加 panel 只动这里**。
///
/// command_id 必须与 panel_str_id 形成 `panel.toggle.<id>` 规则 ——
/// 写在表里而不是运行时拼接，是为了让 catalog 直接出口 `&'static str`
/// 常量供宿主反向引用（例如 glyph 用它标注命令）。
const PANELS: &[PanelEntry] = &[
    PanelEntry {
        panel_str_id: "file_tree",
        command_id: TOGGLE_FILE_TREE,
        title: "切换面板：文件树",
        default_chord: "mod-shift-e",
    },
    PanelEntry {
        panel_str_id: "version_control",
        command_id: TOGGLE_VERSION_CONTROL,
        title: "切换面板：版本管理",
        default_chord: "mod-shift-g",
    },
    PanelEntry {
        panel_str_id: "outline",
        command_id: TOGGLE_OUTLINE,
        title: "切换面板：大纲",
        default_chord: "mod-shift-o",
    },
    PanelEntry {
        panel_str_id: "project_search",
        command_id: TOGGLE_PROJECT_SEARCH,
        title: "切换面板：项目搜索",
        default_chord: "mod-shift-f",
    },
    PanelEntry {
        panel_str_id: "terminal",
        command_id: TOGGLE_TERMINAL,
        title: "切换面板：终端",
        default_chord: "mod-j",
    },
    PanelEntry {
        panel_str_id: "debug",
        command_id: TOGGLE_DEBUG,
        title: "切换面板：调试",
        default_chord: "mod-shift-d",
    },
    PanelEntry {
        panel_str_id: "keyboard_shortcuts",
        command_id: TOGGLE_KEYBOARD_SHORTCUTS,
        title: "切换面板：快捷键",
        default_chord: "mod-shift-k",
    },
];

// ==================================================
// 命令 id 常量 —— glyph / 命令面板 / 菜单引用这些，而不是再手写字符串
// ==================================================

pub const TOGGLE_FILE_TREE: &str = "panel.toggle.file_tree";
pub const TOGGLE_VERSION_CONTROL: &str = "panel.toggle.version_control";
pub const TOGGLE_OUTLINE: &str = "panel.toggle.outline";
pub const TOGGLE_PROJECT_SEARCH: &str = "panel.toggle.project_search";
pub const TOGGLE_TERMINAL: &str = "panel.toggle.terminal";
pub const TOGGLE_DEBUG: &str = "panel.toggle.debug";
pub const TOGGLE_KEYBOARD_SHORTCUTS: &str = "panel.toggle.keyboard_shortcuts";

/// typed builder：以编程方式触发某个 panel 切换。
///
/// `panel_str_id` 必须与 [`PANELS`] 表里某行对得上 —— 宿主侧通常通过
/// `PanelId::as_command_str()` 之类的方法拿到。
#[allow(dead_code)]
pub fn toggle(panel_str_id: &str) -> Option<Invocation> {
    PANELS
        .iter()
        .find(|p| p.panel_str_id == panel_str_id)
        .map(|p| (cid(p.command_id), CommandArgs::new()))
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    for entry in PANELS {
        let panel = entry.panel_str_id.to_string();
        registry
            .install(
                keymap,
                entry.command_id,
                entry.title,
                Box::new(move |ctx, args| {
                    NoArgs::try_from(args)?;
                    ctx.effects.push(HostEffect::TogglePanel(panel.clone()));
                    Ok(CommandOutcome::default())
                }),
            )
            .key(entry.default_chord);
    }
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
