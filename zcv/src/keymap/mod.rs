//! keymap —— 快捷键加载器。
//!
//! 编译时通过 `include_str!` 嵌入 JSON，运行时解析后生成 `KeyBindingsRef`，其中包含供 `cx.bind_keys()` 注册的 `Vec<KeyBinding>` 和供 UI 反向查询的快捷键映射。
//! 调用方应将返回的 `KeyBindingsRef` 注入 GPUI Global，供各组件访问。

use std::collections::{BTreeMap, HashMap};

use gpui::KeyBinding;
use serde::Deserialize;

use crate::project_tree::{TreeActivate, TreeCollapse, TreeExpand, TreeSelectNext, TreeSelectPrev};
use crate::recent_projects::{OpenLocalProject, ToggleProjectPicker};
use crate::ui::{PickerCancel, PickerConfirm, PickerSelectNext, PickerSelectPrev};
use crate::workspace::{
    CloseTab, MinimizeWindow, NextTab, OpenSettings, PrevTab, QuitWindow, Save, ToggleDebug,
    ToggleDiagnostics, ToggleKeyboardShortcuts, ToggleLanguageServer, ToggleMaximizeWindow,
    ToggleOutline, ToggleProjectSearch, ToggleProjectTree, ToggleTerminal, ToggleVersionControl,
};
use zcv_editor::{
    Backspace, Copy, Cut, Delete, Indent, MoveDown, MoveLeft, MoveRight, MoveToBeginningOfLine,
    MoveToEndOfLine, MoveToNextWord, MoveToPreviousWord, MoveUp, Newline, Outdent, Paste, Redo,
    SelectAll, SelectDown, SelectLeft, SelectRight, SelectToBeginningOfLine, SelectToEndOfLine,
    SelectToNextWord, SelectToPreviousWord, SelectUp, Undo,
};

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
pub(crate) fn load() -> KeyBindings {
    let json = platform_json();
    let groups: Vec<RawBindingGroup> =
        serde_json::from_str(json).expect("默认 keymap JSON 格式错误");

    detect_conflicts(&groups);

    let mut bindings = Vec::new();
    let mut shortcuts = HashMap::new();

    for group in groups {
        let context = group.context.as_deref();
        for (keys, action_name) in &group.bindings {
            // 构建反向索引：从 JSON 自动生成，与快捷键定义保持完全一致
            shortcuts
                .entry(action_name.clone())
                .or_insert_with(|| keys.clone());

            if let Some(binding) = build(keys, action_name, context) {
                bindings.push(binding);
            }
        }
    }

    KeyBindings {
        bindings,
        shortcuts,
    }
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
fn platform_json() -> &'static str {
    #[cfg(target_os = "macos")]
    return include_str!("../../assets/keymaps/default-macos.json");
    #[cfg(target_os = "windows")]
    return include_str!("../../assets/keymaps/default-windows.json");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return include_str!("../../assets/keymaps/default-linux.json");
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

/// 根据 action 名称构建 KeyBinding。
fn build(keys: &str, action_name: &str, context: Option<&str>) -> Option<KeyBinding> {
    let binding = match action_name {
        // editor
        "editor::Undo" => KeyBinding::new(keys, Undo, context),
        "editor::Redo" => KeyBinding::new(keys, Redo, context),
        "editor::Cut" => KeyBinding::new(keys, Cut, context),
        "editor::Copy" => KeyBinding::new(keys, Copy, context),
        "editor::Paste" => KeyBinding::new(keys, Paste, context),
        "editor::MoveLeft" => KeyBinding::new(keys, MoveLeft, context),
        "editor::MoveRight" => KeyBinding::new(keys, MoveRight, context),
        "editor::MoveUp" => KeyBinding::new(keys, MoveUp, context),
        "editor::MoveDown" => KeyBinding::new(keys, MoveDown, context),
        "editor::MoveToPreviousWord" => KeyBinding::new(keys, MoveToPreviousWord, context),
        "editor::MoveToNextWord" => KeyBinding::new(keys, MoveToNextWord, context),
        "editor::MoveToBeginningOfLine" => KeyBinding::new(keys, MoveToBeginningOfLine, context),
        "editor::MoveToEndOfLine" => KeyBinding::new(keys, MoveToEndOfLine, context),
        "editor::SelectLeft" => KeyBinding::new(keys, SelectLeft, context),
        "editor::SelectRight" => KeyBinding::new(keys, SelectRight, context),
        "editor::SelectUp" => KeyBinding::new(keys, SelectUp, context),
        "editor::SelectDown" => KeyBinding::new(keys, SelectDown, context),
        "editor::SelectToPreviousWord" => KeyBinding::new(keys, SelectToPreviousWord, context),
        "editor::SelectToNextWord" => KeyBinding::new(keys, SelectToNextWord, context),
        "editor::SelectToBeginningOfLine" => {
            KeyBinding::new(keys, SelectToBeginningOfLine, context)
        }
        "editor::SelectToEndOfLine" => KeyBinding::new(keys, SelectToEndOfLine, context),
        "editor::SelectAll" => KeyBinding::new(keys, SelectAll, context),
        "editor::Backspace" => KeyBinding::new(keys, Backspace, context),
        "editor::Delete" => KeyBinding::new(keys, Delete, context),
        "editor::Newline" => KeyBinding::new(keys, Newline, context),
        "editor::Indent" => KeyBinding::new(keys, Indent, context),
        "editor::Outdent" => KeyBinding::new(keys, Outdent, context),
        // project_tree
        "project_tree::TreeSelectPrev" => KeyBinding::new(keys, TreeSelectPrev, context),
        "project_tree::TreeSelectNext" => KeyBinding::new(keys, TreeSelectNext, context),
        "project_tree::TreeCollapse" => KeyBinding::new(keys, TreeCollapse, context),
        "project_tree::TreeExpand" => KeyBinding::new(keys, TreeExpand, context),
        "project_tree::TreeActivate" => KeyBinding::new(keys, TreeActivate, context),
        // picker
        "picker::PickerSelectPrev" => KeyBinding::new(keys, PickerSelectPrev, context),
        "picker::PickerSelectNext" => KeyBinding::new(keys, PickerSelectNext, context),
        "picker::PickerConfirm" => KeyBinding::new(keys, PickerConfirm, context),
        "picker::PickerCancel" => KeyBinding::new(keys, PickerCancel, context),
        "project_picker::OpenLocalProject" => KeyBinding::new(keys, OpenLocalProject, context),
        // pane
        "pane::PrevTab" => KeyBinding::new(keys, PrevTab, context),
        "pane::NextTab" => KeyBinding::new(keys, NextTab, context),
        "pane::CloseTab" => KeyBinding::new(keys, CloseTab, context),
        // workspace (global, no context)
        "workspace::QuitWindow" => KeyBinding::new(keys, QuitWindow, context),
        "workspace::MinimizeWindow" => KeyBinding::new(keys, MinimizeWindow, context),
        "workspace::ToggleMaximizeWindow" => KeyBinding::new(keys, ToggleMaximizeWindow, context),
        "project_picker::ToggleProjectPicker" => {
            KeyBinding::new(keys, ToggleProjectPicker, context)
        }
        "workspace::OpenSettings" => KeyBinding::new(keys, OpenSettings, context),
        "workspace::ToggleProjectTree" => KeyBinding::new(keys, ToggleProjectTree, context),
        "workspace::ToggleVersionControl" => KeyBinding::new(keys, ToggleVersionControl, context),
        "workspace::ToggleOutline" => KeyBinding::new(keys, ToggleOutline, context),
        "workspace::ToggleLanguageServer" => KeyBinding::new(keys, ToggleLanguageServer, context),
        "workspace::ToggleDiagnostics" => KeyBinding::new(keys, ToggleDiagnostics, context),
        "workspace::ToggleProjectSearch" => KeyBinding::new(keys, ToggleProjectSearch, context),
        "workspace::ToggleTerminal" => KeyBinding::new(keys, ToggleTerminal, context),
        "workspace::ToggleDebug" => KeyBinding::new(keys, ToggleDebug, context),
        "workspace::ToggleKeyboardShortcuts" => {
            KeyBinding::new(keys, ToggleKeyboardShortcuts, context)
        }
        "workspace::Save" => KeyBinding::new(keys, Save, context),
        _ => {
            eprintln!("未知 action 名称: {action_name}");
            return None;
        }
    };
    Some(binding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_macos_keymap_defines_workspace_save_shortcut() {
        let expected_keys = "cmd-s";
        let groups: Vec<RawBindingGroup> =
            serde_json::from_str(include_str!("../../assets/keymaps/default-macos.json"))
                .expect("macOS 默认 keymap JSON 应合法");
        assert!(groups.iter().any(|group| {
            group.context.is_none()
                && group.bindings.get(expected_keys).map(String::as_str) == Some("workspace::Save")
        }));
        assert!(build(expected_keys, "workspace::Save", Some("Workspace")).is_some());
    }

    #[test]
    fn every_platform_keymap_is_nonempty_valid_and_defines_save() {
        for json in [
            include_str!("../../assets/keymaps/default-macos.json"),
            include_str!("../../assets/keymaps/default-linux.json"),
            include_str!("../../assets/keymaps/default-windows.json"),
        ] {
            let groups: Vec<RawBindingGroup> =
                serde_json::from_str(json).expect("平台 keymap 必须是合法 JSON");
            assert!(!groups.is_empty());
            assert!(groups.iter().any(|group| {
                group
                    .bindings
                    .values()
                    .any(|action| action == "workspace::Save")
            }));
        }
    }
}
