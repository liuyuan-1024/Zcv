//! 快捷键加载与解析。
//!
//! 内置平台快捷键经 GPUI action registry 解析为 [`KeyBindings`]，供应用注册和 UI 反向查询。
//! keymap 文件支持 JSONC 风格的 `//` 行注释。

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use anyhow::{Context as _, Result, anyhow};
use gpui::{Action, App, KeyBinding, KeyBindingContextPredicate};
use serde::Deserialize;
use serde_json::Value;

// ── 公开类型 ─────────────────────────────────────────────────────────

/// 快捷键绑定集合：正向（注册） + 反向（查询）。
pub struct KeyBindings {
    pub bindings: Vec<KeyBinding>,
    shortcuts: Vec<(Box<dyn Action>, String)>,
}

impl KeyBindings {
    /// 根据完整 action（包括参数）查询当前平台的快捷键显示字符串。
    ///
    /// - macOS：修饰键显示为符号（`cmd-shift-e` → `⌘⇧E`）
    /// - Linux / Windows：修饰键显示为文本（`ctrl-shift-e` → `Ctrl+Shift+E`）
    pub fn display_shortcut(&self, action: &dyn Action) -> Option<String> {
        self.shortcuts
            .iter()
            .find(|(candidate, _)| candidate.partial_eq(action))
            .map(|(_, keys)| display_format(keys))
    }

    /// 仅在调用方没有 Action 实例时按名称查询。
    pub fn display_shortcut_named(&self, action_name: &str) -> Option<String> {
        self.shortcuts
            .iter()
            .find(|(action, _)| action.name() == action_name)
            .map(|(_, keys)| display_format(keys))
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
            "ctrl" | "control" => Some("⌃"),
            "shift" => Some("⇧"),
            "option" | "alt" => Some("⌥"),
            "cmd" | "command" => Some("⌘"),
            _ => None,
        }
    }

    /// 功能键的 macOS 键帽符号（对齐 Apple 官方键盘符号表）。
    fn key_symbol(key: &str) -> Option<&'static str> {
        match key {
            "backspace" => Some("⌫"),
            "delete" => Some("⌦"),
            "enter" | "return" => Some("↩"),
            "escape" => Some("⎋"),
            "tab" => Some("⇥"),
            "capslock" => Some("⇪"),
            "up" => Some("↑"),
            "down" => Some("↓"),
            "left" => Some("←"),
            "right" => Some("→"),
            "home" => Some("↖"),
            "end" => Some("↘"),
            "pageup" => Some("⇞"),
            "pagedown" => Some("⇟"),
            "space" => Some("␣"),
            _ => None,
        }
    }

    // chord 段（如 `ctrl-k ctrl-s`）用空格分隔，段内按键直接拼接。
    raw.split_whitespace()
        .map(|chord| {
            chord
                .split('-')
                .map(|part| {
                    modifier(part)
                        .map(|s| s.to_string())
                        .or_else(|| key_symbol(part).map(|s| s.to_string()))
                        .unwrap_or_else(|| {
                            if part.len() == 1 {
                                part.to_uppercase()
                            } else {
                                part.to_string()
                            }
                        })
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Linux / Windows：`ctrl-shift-e` → `Ctrl+Shift+E`
#[cfg(not(target_os = "macos"))]
fn text_display(raw: &str) -> String {
    fn modifier(key: &str) -> Option<&'static str> {
        match key {
            "cmd" | "ctrl" => Some("Ctrl"),
            "shift" => Some("Shift"),
            "alt" | "option" => Some("Alt"),
            "super" => Some("Super"),
            "win" => Some("Win"),
            _ => None,
        }
    }

    // chord 段（如 `ctrl-k ctrl-s`）用空格分隔，段内按键用 `+` 连接。
    raw.split_whitespace()
        .map(|chord| {
            chord
                .split('-')
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
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl gpui::Global for KeyBindings {}

// ── 公开函数 ─────────────────────────────────────────────────────────

/// 在应用启动阶段加载并注册当前平台的内置快捷键。
///
/// GPUI 负责命令分发，`KeyBindings` 全局值负责向按钮和菜单提供快捷键提示；
/// 两者必须来自同一次加载，避免注册行为与界面提示使用不同数据源。
pub fn init(cx: &mut App) -> Result<()> {
    let keybindings = load(cx)?;
    cx.bind_keys(keybindings.bindings.clone());
    cx.set_global(keybindings);
    Ok(())
}

/// 加载当前平台的内置 keymap。
fn load(cx: &App) -> Result<KeyBindings> {
    let (source, json) = platform_keymap()?;
    load_json(source, &json, cx)
}

pub(crate) fn load_json(source: &str, json: &str, cx: &App) -> Result<KeyBindings> {
    let groups: Vec<RawBindingGroup> = serde_json::from_str(&strip_line_comments(json))
        .with_context(|| format!("{source} 不是合法的 keymap JSON"))?;

    detect_conflicts(&groups);

    let mut bindings = Vec::new();
    let mut shortcuts: Vec<(Box<dyn Action>, String)> = Vec::new();

    for group in groups {
        let context = group
            .context
            .as_deref()
            .map(KeyBindingContextPredicate::parse)
            .transpose()
            .map_err(|error| anyhow!("{source} 包含非法快捷键上下文 {:?}：{error}", group.context))?
            .map(Rc::new);

        for (keys, raw_action) in &group.bindings {
            let action_name = raw_action.name();
            let action = cx
                .build_action(action_name, raw_action.params().cloned())
                .with_context(|| {
                    format!("{source} 的快捷键 {keys:?} 引用了未知或无效 action {action_name:?}")
                })?;
            let shortcut_action = action.boxed_clone();
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
            if !shortcuts
                .iter()
                .any(|(candidate, _)| candidate.partial_eq(shortcut_action.as_ref()))
            {
                shortcuts.push((shortcut_action, keys.clone()));
            }
        }
    }

    Ok(KeyBindings {
        bindings,
        shortcuts,
    })
}

// ── 私有辅助函数 ─────────────────────────────────────────────────────

/// 去除 JSONC 风格的 `//` 行注释，字符串内的 `//`（如 URL）不受影响。
fn strip_line_comments(json: &str) -> Cow<'_, str> {
    if !json.contains("//") {
        return Cow::Borrowed(json);
    }

    let mut result = String::with_capacity(json.len());
    let mut chars = json.chars().peekable();
    let mut in_string = false;

    while let Some(ch) = chars.next() {
        if in_string {
            result.push(ch);
            match ch {
                '\\' => {
                    // 转义字符与下一个字符一并保留，避免误判 \" 为字符串结束。
                    if let Some(&escaped) = chars.peek() {
                        chars.next();
                        result.push(escaped);
                    }
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                result.push(ch);
            }
            '/' if chars.peek() == Some(&'/') => {
                // 丢弃注释直到行尾，保留换行符以维持行号。
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        result.push('\n');
                        break;
                    }
                }
            }
            _ => result.push(ch),
        }
    }

    Cow::Owned(result)
}

/// JSON 文件的顶层结构：一组快捷键分组。
#[derive(Deserialize)]
struct RawBindingGroup {
    #[serde(default)]
    context: Option<String>,
    /// 键位字符串 → action 名称的映射
    bindings: BTreeMap<String, RawAction>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawAction {
    Name(String),
    WithParams((String, Value)),
}

impl RawAction {
    fn name(&self) -> &str {
        match self {
            Self::Name(name) | Self::WithParams((name, _)) => name,
        }
    }

    fn params(&self) -> Option<&Value> {
        match self {
            Self::Name(_) => None,
            Self::WithParams((_, params)) => Some(params),
        }
    }
}

/// 解析当前平台的唯一内置快捷键资源。
fn platform_keymap() -> Result<(&'static str, Cow<'static, str>)> {
    #[cfg(target_os = "macos")]
    let source = "default-macos.json";
    #[cfg(target_os = "windows")]
    let source = "default-windows.json";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let source = "default-linux.json";

    let json = zcv_assets::text(&format!("keymaps/{source}"))
        .with_context(|| format!("缺少内置快捷键 {source}"))?;
    Ok((source, json))
}

/// 检测同一 (键位, 上下文) 被映射到不同 action 的冲突并告警。
fn detect_conflicts(groups: &[RawBindingGroup]) {
    let mut seen: HashMap<(&str, Option<&str>), &RawAction> = HashMap::new();
    for group in groups {
        let context = group.context.as_deref();
        for (keys, action) in &group.bindings {
            if let Some(prev) = seen.get(&(keys.as_str(), context)) {
                eprintln!(
                    "快捷键冲突: {keys:15} (ctx: {ctx:12}) → '{}' 和 '{}'，后者覆盖前者",
                    prev.name(),
                    action.name(),
                    ctx = context.unwrap_or("(全局)"),
                );
            }
            seen.insert((keys, context), action);
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use zcv_actions::FocusOrHidePanel;

    use super::*;

    /// 三个平台的内置 keymap 都必须能构建，且引用的 action 已注册（集成校验：注册来自 zcv-actions）。
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
    fn parameterized_actions_have_distinct_shortcuts(cx: &mut TestAppContext) {
        let keybindings = cx.update(|cx| {
            load_json(
                "parameterized.json",
                r#"[{"bindings":{
                    "cmd-shift-e":["dock::FocusOrHidePanel",{"panel":"project-tree"}],
                    "cmd-shift-g":["dock::FocusOrHidePanel",{"panel":"version-control"}]
                }}]"#,
                cx,
            )
            .unwrap()
        });

        assert_eq!(
            keybindings.display_shortcut(&FocusOrHidePanel::new("project-tree")),
            Some(display_format("cmd-shift-e"))
        );
        assert_eq!(
            keybindings.display_shortcut(&FocusOrHidePanel::new("version-control")),
            Some(display_format("cmd-shift-g"))
        );
    }

    /// 项目/分支选择器分组使用的复合 context 必须可解析。
    #[test]
    fn composite_context_parses() {
        KeyBindingContextPredicate::parse(
            "Picker || (ProjectPicker > Picker > Editor) || (BranchPicker > Picker > Editor)",
        )
        .expect("复合 context 必须可解析");
    }

    /// 读取内置 keymap 并按 JSONC 语义解析（支持 `//` 行注释）。
    fn parse_builtin_keymap(source: &str) -> Vec<RawBindingGroup> {
        let json = zcv_assets::text(&format!("keymaps/{source}"))
            .unwrap_or_else(|_| panic!("缺少内置快捷键 {source}"));
        serde_json::from_str(&strip_line_comments(&json)).expect("keymap 必须是合法 JSON")
    }

    /// 行注释在解析前被剔除；字符串内的 `//` 与转义引号不受影响。
    #[test]
    fn line_comments_are_stripped_before_parsing() {
        let stripped = strip_line_comments(
            r#"// 头部注释
{
    "url": "https://example.com", // 行尾注释
    "escaped": "a\"b // 不是注释"
}"#,
        );
        assert!(stripped.contains("https://example.com"));
        assert!(stripped.contains("a\\\"b // 不是注释"));
        assert!(!stripped.contains("头部注释"));
        assert!(!stripped.contains("行尾注释"));
        serde_json::from_str::<Value>(&stripped).expect("剥离后应为合法 JSON");
    }

    /// 内置 keymap 的所有 chord 段必须能被 gpui 解析，否则加载时会失败。
    #[test]
    fn all_builtin_keymap_keystrokes_parse() {
        for source in [
            "default-macos.json",
            "default-linux.json",
            "default-windows.json",
        ] {
            let groups = parse_builtin_keymap(source);
            for group in &groups {
                for keys in group.bindings.keys() {
                    for keystroke in keys.split_whitespace() {
                        gpui::Keystroke::parse(keystroke).unwrap_or_else(|error| {
                            panic!("{source} 的键位 {keys:?} 无法解析：{error}")
                        });
                    }
                }
            }
        }
    }

    #[test]
    fn default_keymap_resolves_to_platform_asset() {
        let (default_source, _) = platform_keymap().unwrap();
        #[cfg(target_os = "macos")]
        assert_eq!(default_source, "default-macos.json");
        #[cfg(target_os = "windows")]
        assert_eq!(default_source, "default-windows.json");
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(default_source, "default-linux.json");
    }

    #[gpui::test]
    fn init_registers_bindings_and_shortcut_queries(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init(cx).unwrap();
            let keybindings = cx.global::<KeyBindings>();
            #[cfg(target_os = "macos")]
            assert_eq!(
                keybindings.display_shortcut_named("workspace::Save"),
                Some("⌘S".to_string())
            );
            #[cfg(not(target_os = "macos"))]
            assert_eq!(
                keybindings.display_shortcut_named("workspace::Save"),
                Some("Ctrl+S".to_string())
            );
        });
    }

    /// 提交快捷键必须限制在版本控制上下文，并遵循各平台主修饰键约定。
    #[test]
    fn version_control_keymap_binds_commit_on_every_platform() {
        for (source, keys) in [
            ("default-macos.json", "cmd-enter"),
            ("default-linux.json", "ctrl-enter"),
            ("default-windows.json", "ctrl-enter"),
        ] {
            let groups = parse_builtin_keymap(source);
            let version_control = groups
                .iter()
                .find(|group| group.context.as_deref() == Some("VersionControl"))
                .unwrap_or_else(|| panic!("{source} 缺少 VersionControl 上下文"));
            assert_eq!(
                version_control.bindings.get(keys).map(RawAction::name),
                Some("version_control::Commit"),
                "{source} 的 {keys} 应提交当前暂存"
            );
        }
    }

    /// macOS 显示格式：修饰键与功能键都使用键帽符号。
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_display_uses_key_cap_symbols() {
        assert_eq!(macos_display("cmd-backspace"), "⌘⌫");
        assert_eq!(macos_display("cmd-shift-e"), "⌘⇧E");
        assert_eq!(macos_display("ctrl-alt-delete"), "⌃⌥⌦");
        assert_eq!(macos_display("shift-pageup"), "⇧⇞");
        assert_eq!(macos_display("cmd-enter"), "⌘↩");
        assert_eq!(macos_display("alt-left"), "⌥←");
        assert_eq!(macos_display("shift-tab"), "⇧⇥");
        assert_eq!(macos_display("cmd-space"), "⌘␣");
        assert_eq!(macos_display("cmd-a"), "⌘A");
        // chord 段用空格分隔，不粘连
        assert_eq!(macos_display("ctrl-k ctrl-s"), "⌃K ⌃S");
    }

    /// Editor 上下文必须始终覆盖行首尾选择绑定，防止 keymap 编辑时被意外删除。
    #[gpui::test]
    fn editor_keymap_covers_line_selection_extensions(cx: &mut TestAppContext) {
        cx.update(|_cx| {
            for source in [
                "default-macos.json",
                "default-linux.json",
                "default-windows.json",
            ] {
                let groups = parse_builtin_keymap(source);
                let editor = groups
                    .iter()
                    .find(|group| group.context.as_deref() == Some("Editor"))
                    .unwrap_or_else(|| panic!("{source} 缺少 Editor 上下文"));
                for (keys, action) in [
                    ("shift-home", "editor::SelectToBeginningOfLine"),
                    ("shift-end", "editor::SelectToEndOfLine"),
                ] {
                    assert_eq!(
                        editor.bindings.get(keys).map(RawAction::name),
                        Some(action),
                        "{source} 的 {keys} 应绑定 {action}"
                    );
                }
            }
        });
    }

    /// Ctrl-C 在终端中必须发送中断字符，不能落到编辑器复制或全局取消动作。
    #[test]
    fn terminal_keymap_binds_ctrl_c_to_interrupt_on_every_platform() {
        for source in [
            "default-macos.json",
            "default-linux.json",
            "default-windows.json",
        ] {
            let groups = parse_builtin_keymap(source);
            let terminal = groups
                .iter()
                .find(|group| group.context.as_deref() == Some("terminal"))
                .unwrap_or_else(|| panic!("{source} 缺少 terminal 上下文"));
            assert_eq!(
                terminal.bindings.get("ctrl-c").map(RawAction::name),
                Some("terminal::Interrupt"),
                "{source} 的 Ctrl-C 必须发送终端中断"
            );
        }
    }
}
