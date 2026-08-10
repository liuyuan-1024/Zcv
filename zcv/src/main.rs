#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod active_buffer_language;
mod breadcrumbs;
mod cursor_position;
mod diagnostics;
mod language_tools;
mod paths;
mod preview;
mod project_search;
mod project_tree;
mod settings;
mod version_control;
mod workspace;

use std::path::PathBuf;

use gpui::{App, Application};
use zcv_assets::Assets;

use workspace::open_project_window;

fn main() {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        Assets.load_fonts(cx).expect("内置字体应能注册");

        settings::init(cx);
        preview::init(cx);
        // 注册文件 Item Provider（文本兜底：项目文件 → Editor）。
        zcv_editor::init_item_providers(cx);

        // 初始项目根：开发构建打开当前工作区，正式构建打开启动目录。
        #[cfg(debug_assertions)]
        let initial_root = {
            let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            crate_dir.parent().unwrap_or(&crate_dir).to_path_buf()
        };
        #[cfg(not(debug_assertions))]
        let initial_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        open_project_window(initial_root, cx).expect("主窗口应能创建");

        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use zcv_keymap::load_json;

    /// keymap JSON 引用的所有 action 必须已注册（集成校验：注册来自本 crate 与 zcv-editor）。
    #[gpui::test]
    fn every_platform_keymap_builds_every_registered_action(cx: &mut TestAppContext) {
        cx.update(|cx| {
            for (source, json) in [
                (
                    "default-macos.json",
                    zcv_assets::text("keymaps/default-macos.json")
                        .expect("内置 macOS 快捷键应存在"),
                ),
                (
                    "default-linux.json",
                    zcv_assets::text("keymaps/default-linux.json")
                        .expect("内置 Linux 快捷键应存在"),
                ),
                (
                    "default-windows.json",
                    zcv_assets::text("keymaps/default-windows.json")
                        .expect("内置 Windows 快捷键应存在"),
                ),
            ] {
                let keybindings =
                    load_json(source, &json, cx).expect("每个平台的全部内置绑定都应能构建");
                assert!(!keybindings.bindings.is_empty());
                assert!(
                    cx.build_action("workspace::Save", None).is_ok(),
                    "workspace::Save 应已注册且 keymap 可引用"
                );
            }
        });
    }

    /// 面板切换 action 只能由 dock 持有（防止 status_bar 等重复注册导致快捷键歧义）。
    #[gpui::test]
    fn panel_toggle_actions_are_owned_only_by_dock(cx: &mut TestAppContext) {
        cx.update(|cx| {
            for action in [
                "ToggleProjectTree",
                "ToggleVersionControl",
                "ToggleOutline",
                "ToggleLanguageServer",
                "ToggleDiagnostics",
                "ToggleProjectSearch",
                "ToggleTerminal",
                "ToggleDebug",
                "ToggleKeyboardShortcuts",
            ] {
                assert!(cx.build_action(&format!("dock::{action}"), None).is_ok());
                assert!(
                    cx.build_action(&format!("status_bar::{action}"), None)
                        .is_err()
                );
            }
        });
    }
}
