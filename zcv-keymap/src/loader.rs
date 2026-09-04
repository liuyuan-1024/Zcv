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

    /// 功能键的 macOS 键帽符号（键帽符号数据来源：Apple 官方键盘符号表）。
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
        // 无上下文的默认组挂在 Workspace 根上下文：它在焦点链中最浅，因而任何面板或内容的专属上下文都会优先于默认快捷键。
        let context_source = group.context.as_deref().unwrap_or("Workspace");
        let context = KeyBindingContextPredicate::parse(context_source).map_err(|error| {
            anyhow!("{source} 包含非法快捷键上下文 {:?}：{error}", group.context)
        })?;
        let context = Rc::new(context);

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
                Some(context.clone()),
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
    use gpui::{KeyContext, Keymap, Keystroke, TestAppContext};
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
            "Picker || (RecentProjects > Picker > Editor) || (GitBranchSelector > Picker > Editor)",
        )
        .expect("复合 context 必须可解析");
        KeyBindingContextPredicate::parse(
            "(BufferSearchBar || ProjectSearchBar) && in_replace > Editor",
        )
        .expect("搜索条复合 context 必须可解析");
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

    /// 提交快捷键必须覆盖版本控制变更树和提交信息编辑器，并遵循各平台主修饰键约定。
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
                .find(|group| group.context.as_deref() == Some("GitPanel"))
                .unwrap_or_else(|| panic!("{source} 缺少版本控制提交上下文"));
            assert_eq!(
                version_control.bindings.get(keys).map(RawAction::name),
                Some("version_control::Commit"),
                "{source} 的 {keys} 应提交当前暂存"
            );
        }
    }

    /// 替换框的 Enter 语义由 in_replace 标签分组声明，不得缺失或退化。
    #[test]
    fn search_replace_input_enter_is_declared_by_in_replace_on_every_platform() {
        for source in [
            "default-macos.json",
            "default-linux.json",
            "default-windows.json",
        ] {
            let groups = parse_builtin_keymap(source);
            let in_replace = groups
                .iter()
                .find(|group| {
                    group.context.as_deref()
                        == Some("(BufferSearchBar || ProjectSearchBar) && in_replace > Editor")
                })
                .unwrap_or_else(|| panic!("{source} 缺少替换框 in_replace 上下文"));
            assert_eq!(
                in_replace.bindings.get("enter").map(RawAction::name),
                Some("search::ReplaceNext"),
                "{source} 的替换框 Enter 应替换当前匹配"
            );
        }
    }

    /// 变更树快捷键不得泄漏到同一面板内的提交信息编辑器。
    #[test]
    fn version_control_tree_keymap_is_scoped_on_every_platform() {
        for source in [
            "default-macos.json",
            "default-linux.json",
            "default-windows.json",
        ] {
            let groups = parse_builtin_keymap(source);
            let changes_tree = groups
                .iter()
                .find(|group| group.context.as_deref() == Some("GitPanel && ChangesList"))
                .unwrap_or_else(|| panic!("{source} 缺少版本控制变更树上下文"));
            assert_eq!(
                changes_tree.bindings.get("space").map(RawAction::name),
                Some("version_control::ToggleStaged"),
                "{source} 的空格键只应在版本控制变更树内切换暂存"
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
                .find(|group| group.context.as_deref() == Some("Terminal"))
                .unwrap_or_else(|| panic!("{source} 缺少 Terminal 上下文"));
            assert_eq!(
                terminal.bindings.get("ctrl-c").map(RawAction::name),
                Some("terminal::Interrupt"),
                "{source} 的 Ctrl-C 必须发送终端中断"
            );
        }
    }

    /// 内容与 UI 缩放均为全局快捷键；终端在自身上下文覆盖内容缩放键位。
    #[test]
    fn font_size_keymaps_keep_content_ui_and_terminal_scopes_distinct() {
        for (source, content_keys, ui_keys, terminal_keys) in [
            ("default-macos.json", "cmd-=", "cmd-shift-=", "cmd-="),
            ("default-linux.json", "ctrl-=", "ctrl-shift-=", "ctrl-="),
            ("default-windows.json", "ctrl-=", "ctrl-shift-=", "ctrl-="),
        ] {
            let groups = parse_builtin_keymap(source);
            let global = groups
                .iter()
                .find(|group| group.context.is_none() && group.bindings.contains_key(ui_keys))
                .unwrap_or_else(|| panic!("{source} 缺少工作区 UI 字号绑定"));

            assert_eq!(
                global.bindings.get(content_keys).map(RawAction::name),
                Some("workspace::IncreaseContentFontSize"),
                "{source} 的 {content_keys} 应全局缩放工作区内容"
            );
            assert_eq!(
                global.bindings.get(ui_keys).map(RawAction::name),
                Some("workspace::IncreaseUiFontSize"),
                "{source} 的 {ui_keys} 应缩放工作区 UI"
            );

            let terminal = groups
                .iter()
                .find(|group| group.context.as_deref() == Some("Terminal"))
                .unwrap_or_else(|| panic!("{source} 缺少终端上下文"));
            assert_eq!(
                terminal.bindings.get(terminal_keys).map(RawAction::name),
                Some("terminal::IncreaseFontSize"),
                "{source} 的 {terminal_keys} 在终端中应只缩放终端字号"
            );
        }
    }

    /// 同一按键同时存在全局与终端绑定时，终端焦点必须优先调度终端动作。
    #[gpui::test]
    fn terminal_font_size_binding_overrides_global_binding(cx: &mut TestAppContext) {
        cx.update(|cx| {
            for (source, keys) in [
                ("default-macos.json", "cmd-="),
                ("default-linux.json", "ctrl-="),
                ("default-windows.json", "ctrl-="),
            ] {
                let json = zcv_assets::text(&format!("keymaps/{source}"))
                    .unwrap_or_else(|_| panic!("缺少内置快捷键 {source}"));
                let keybindings = load_json(source, &json, cx)
                    .unwrap_or_else(|error| panic!("{source} 应能加载：{error}"));
                let keymap = Keymap::new(keybindings.bindings);
                let (bindings, _) = keymap.bindings_for_input(
                    &[Keystroke::parse(keys).expect("字号快捷键应合法")],
                    &[
                        KeyContext::parse("Workspace").expect("工作区上下文应合法"),
                        KeyContext::parse("Terminal").expect("终端上下文应合法"),
                    ],
                );

                assert_eq!(
                    bindings.first().map(|binding| binding.action().name()),
                    Some("terminal::IncreaseFontSize"),
                    "{source} 的 {keys} 在终端聚焦时应优先调整终端字号"
                );
            }
        });
    }

    /// 编辑区与终端共用 Pane 标签切换键位，不在终端上下文维护第二套规则。
    #[test]
    fn macos_pane_keymap_binds_adjacent_tabs_to_cmd_brackets() {
        let groups = parse_builtin_keymap("default-macos.json");
        let pane = groups
            .iter()
            .find(|group| group.context.as_deref() == Some("Pane"))
            .expect("macOS 快捷键应包含 Pane 上下文");

        assert_eq!(
            pane.bindings.get("cmd-[").map(RawAction::name),
            Some("pane::PrevTab")
        );
        assert_eq!(
            pane.bindings.get("cmd-]").map(RawAction::name),
            Some("pane::NextTab")
        );
    }
}
