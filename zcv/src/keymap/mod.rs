//! keymap —— 快捷键加载器。
//!
//! 编译时通过 `include_str!` 嵌入 JSON，运行时通过 GPUI action registry 解析为[`KeyBindings`]，供应用注册和 UI 反向查询。

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use anyhow::{Context as _, Result, anyhow};
use gpui::{App, KeyBinding, KeyBindingContextPredicate};
use serde::Deserialize;

// ── 公开类型 ─────────────────────────────────────────────────────────

/// 快捷键绑定集合：正向（注册） + 反向（查询）。
pub(crate) struct KeyBindings {
    pub(crate) bindings: Vec<KeyBinding>,
    shortcuts: HashMap<String, String>, // action名 → 键位字符串
}

impl KeyBindings {
    /// 根据 action 名称查询当前平台的快捷键显示字符串。
    ///
    /// - macOS：修饰键显示为符号（`cmd-shift-e` → `⌘⇧E`）
    /// - Linux / Windows：修饰键显示为文本（`ctrl-shift-e` → `Ctrl+Shift+E`）
    pub(crate) fn display_shortcut(&self, action_name: &str) -> Option<String> {
        self.shortcuts.get(action_name).map(|s| display_format(s))
    }
}

/// 将原始快捷键字符串转为当前平台适合显示的格式。
#[cfg(target_os = "macos")]
fn display_format(raw: &str) -> String {
    macos_display(raw)
}

/// 将原始快捷键字符串转为当前平台适合显示的格式。
#[cfg(not(target_os = "macos"))]
fn display_format(raw: &str) -> String {
    text_display(raw)
}

/// macOS：`cmd-shift-e` → `⌘⇧E`
#[cfg(target_os = "macos")]
fn macos_display(raw: &str) -> String {
    fn modifier(key: &str) -> Option<&'static str> {
        match key {
            "cmd" | "command" => Some("⌘"),
            "ctrl" | "control" => Some("⌃"),
            "shift" => Some("⇧"),
            "option" | "alt" => Some("⌥"),
            _ => None,
        }
    }

    raw.split('-')
        .map(|part| {
            modifier(part).map(|s| s.to_string()).unwrap_or_else(|| {
                if part.len() == 1 {
                    part.to_uppercase()
                } else {
                    part.to_string()
                }
            })
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Linux / Windows：`ctrl-shift-e` → `Ctrl+Shift+E`
#[cfg(not(target_os = "macos"))]
fn text_display(raw: &str) -> String {
    fn modifier(key: &str) -> Option<&'static str> {
        match key {
            "cmd" | "ctrl" => Some("Ctrl"),
            "shift" => Some("Shift"),
            "alt" | "option" => Some("Alt"),
            "super" | "win" => Some("Win"),
            _ => None,
        }
    }

    raw.split('-')
        .map(|part| {
            if part == "," {
                return ",".to_string();
            }
            modifier(part).map(|s| s.to_string()).unwrap_or_else(|| {
                if part.len() == 1 {
                    part.to_uppercase()
                } else {
                    part.to_string()
                }
            })
        })
        .collect::<Vec<_>>()
        .join("+")
}

impl gpui::Global for KeyBindings {}

// ── 公开函数 ─────────────────────────────────────────────────────────

/// 加载当前平台默认 keymap。
///
/// 调用方应在 AppView::new 中完成两步：
///   1. `cx.bind_keys(keybindings.bindings.clone())`
///   2. `cx.set_global(keybindings)`
pub(crate) fn load(cx: &App) -> Result<KeyBindings> {
    let (source, json) = platform_keymap();
    load_json(source, json, cx)
}

fn load_json(source: &str, json: &str, cx: &App) -> Result<KeyBindings> {
    let groups: Vec<RawBindingGroup> =
        serde_json::from_str(json).with_context(|| format!("{source} 不是合法的 keymap JSON"))?;

    detect_conflicts(&groups);

    let mut bindings = Vec::new();
    let mut shortcuts = HashMap::new();

    for group in groups {
        let context = group
            .context
            .as_deref()
            .map(KeyBindingContextPredicate::parse)
            .transpose()
            .map_err(|error| anyhow!("{source} 包含非法快捷键上下文 {:?}：{error}", group.context))?
            .map(Rc::new);

        for (keys, action_name) in &group.bindings {
            let action = cx.build_action(action_name, None).with_context(|| {
                format!("{source} 的快捷键 {keys:?} 引用了未知或无效 action {action_name:?}")
            })?;
            let binding = KeyBinding::load(
                keys,
                action,
                context.clone(),
                false,
                None,
                cx.keyboard_mapper().as_ref(),
            )
            .map_err(|error| {
                anyhow!(
                    "{source} 为 action {action_name:?} 配置了非法快捷键 {:?}",
                    error.keystroke
                )
            })?;

            bindings.push(binding);
            shortcuts
                .entry(action_name.clone())
                .or_insert_with(|| keys.clone());
        }
    }

    Ok(KeyBindings {
        bindings,
        shortcuts,
    })
}

// ── 私有辅助函数 ─────────────────────────────────────────────────────

/// JSON 文件的顶层结构：一组快捷键分组。
#[derive(Deserialize)]
struct RawBindingGroup {
    #[serde(default)]
    context: Option<String>,
    /// 键位字符串 → action 名称的映射
    bindings: BTreeMap<String, String>,
}

/// 按当前平台编译对应的默认快捷键文件。
fn platform_keymap() -> (&'static str, &'static str) {
    #[cfg(target_os = "macos")]
    return (
        "default-macos.json",
        include_str!("../../assets/keymaps/default-macos.json"),
    );
    #[cfg(target_os = "windows")]
    return (
        "default-windows.json",
        include_str!("../../assets/keymaps/default-windows.json"),
    );
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return (
        "default-linux.json",
        include_str!("../../assets/keymaps/default-linux.json"),
    );
}

/// 检测同一 (键位, 上下文) 被映射到不同 action 的冲突并告警。
fn detect_conflicts(groups: &[RawBindingGroup]) {
    let mut seen: HashMap<(&str, Option<&str>), &str> = HashMap::new();
    for group in groups {
        let context = group.context.as_deref();
        for (keys, action_name) in &group.bindings {
            if let Some(prev) = seen.get(&(keys.as_str(), context)) {
                eprintln!(
                    "快捷键冲突: {keys:15} (ctx: {ctx:12}) → '{prev}' 和 '{action_name}'，后者覆盖前者",
                    ctx = context.unwrap_or("(全局)"),
                );
            }
            seen.insert((keys, context), action_name);
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn every_platform_keymap_builds_every_registered_action(cx: &mut TestAppContext) {
        cx.update(|cx| {
            for (source, json) in [
                (
                    "default-macos.json",
                    include_str!("../../assets/keymaps/default-macos.json"),
                ),
                (
                    "default-linux.json",
                    include_str!("../../assets/keymaps/default-linux.json"),
                ),
                (
                    "default-windows.json",
                    include_str!("../../assets/keymaps/default-windows.json"),
                ),
            ] {
                let keybindings =
                    load_json(source, json, cx).expect("每个平台的全部内置绑定都应能构建");
                assert!(!keybindings.bindings.is_empty());
                assert!(keybindings.shortcuts.contains_key("workspace::Save"));
            }
        });
    }

    #[gpui::test]
    fn built_in_keymap_rejects_unknown_actions(cx: &mut TestAppContext) {
        let error = cx.update(|cx| {
            load_json(
                "invalid.json",
                r#"[{"bindings":{"ctrl-x":"missing::Action"}}]"#,
                cx,
            )
            .err()
            .expect("未知 action 必须使内置 keymap 加载失败")
        });
        assert!(error.to_string().contains("missing::Action"));
    }

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
