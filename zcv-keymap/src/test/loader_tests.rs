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
                zcv_assets::text("keymaps/default-macos.json").expect("内置 macOS 快捷键应存在"),
            ),
            (
                "default-linux.json",
                zcv_assets::text("keymaps/default-linux.json").expect("内置 Linux 快捷键应存在"),
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
        Some(platform::display_format("cmd-shift-e"))
    );
    assert_eq!(
        keybindings.display_shortcut(&FocusOrHidePanel::new("version-control")),
        Some(platform::display_format("cmd-shift-g"))
    );
}

/// Picker 搜索框使用的嵌套 context 必须可解析。
#[test]
fn composite_context_parses() {
    KeyBindingContextPredicate::parse("Picker > Editor").expect("Picker 嵌套 context 必须可解析");
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
    let (default_source, _) = platform::keymap().unwrap();
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

/// macOS 上会产生字符的 Alt 字母快捷键必须包含 Cmd，避免被输入法优先消费。
#[test]
fn macos_search_shortcuts_keep_cmd_with_alt_letters() {
    let groups = parse_builtin_keymap("default-macos.json");
    let search = groups
        .iter()
        .find(|group| {
            group.context.as_deref()
                == Some("BufferSearchBar || ProjectSearchBar || ProjectDiffSearchBar || GitGraphSearchBar")
        })
        .expect("macOS 搜索上下文应存在");

    for (keys, action) in [
        ("cmd-alt-c", "search::ToggleCaseSensitive"),
        ("cmd-alt-w", "search::ToggleWholeWord"),
        ("cmd-alt-x", "search::ToggleRegex"),
    ] {
        assert_eq!(
            search.bindings.get(keys).map(RawAction::name),
            Some(action),
            "macOS 的 {keys} 应保留 Cmd 以绕过输入法字符处理"
        );
    }

    for keys in ["alt-c", "alt-w", "alt-x"] {
        assert!(
            !search.bindings.contains_key(keys),
            "macOS 不应继续注册会被输入法抢占的 {keys}"
        );
    }
}

#[test]
fn macos_editor_keymap_binds_structured_selection_pair() {
    let groups = parse_builtin_keymap("default-macos.json");
    let editor = groups
        .iter()
        .find(|group| group.context.as_deref() == Some("Editor"))
        .expect("macOS Editor 上下文应存在");

    assert_eq!(
        editor.bindings.get("alt-cmd-right").map(RawAction::name),
        Some("editor::SelectLargerSyntaxNode")
    );
    assert_eq!(
        editor.bindings.get("alt-cmd-left").map(RawAction::name),
        Some("editor::SelectSmallerSyntaxNode")
    );
}

#[test]
fn editor_keymap_binds_f2_to_local_rename_on_every_platform() {
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
        assert_eq!(
            editor.bindings.get("f2").map(RawAction::name),
            Some("editor::RenameLocal"),
            "{source} 的 F2 应打开局部重命名"
        );
    }
}

#[test]
fn single_line_editor_keymap_binds_local_rename_confirmation() {
    for source in [
        "default-macos.json",
        "default-linux.json",
        "default-windows.json",
    ] {
        let groups = parse_builtin_keymap(source);
        let input = groups
            .iter()
            .find(|group| group.context.as_deref() == Some("Editor && mode == single_line"))
            .unwrap_or_else(|| panic!("{source} 缺少单行重命名输入上下文"));
        assert_eq!(
            input.bindings.get("enter").map(RawAction::name),
            Some("editor::ConfirmLocalRename"),
            "{source} 的单行输入 Enter 应提交局部重命名"
        );
        assert_eq!(
            input.bindings.get("escape").map(RawAction::name),
            Some("editor::CancelLocalRename"),
            "{source} 的单行输入 Escape 应取消局部重命名"
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
                    == Some(
                        "(BufferSearchBar || ProjectSearchBar || ProjectDiffSearchBar || GitGraphSearchBar) && in_replace > Editor",
                    )
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
    assert_eq!(platform::macos_display("cmd-backspace"), "⌘⌫");
    assert_eq!(platform::macos_display("cmd-shift-e"), "⌘⇧E");
    assert_eq!(platform::macos_display("ctrl-alt-delete"), "⌃⌥⌦");
    assert_eq!(platform::macos_display("shift-pageup"), "⇧⇞");
    assert_eq!(platform::macos_display("cmd-enter"), "⌘↩");
    assert_eq!(platform::macos_display("alt-left"), "⌥←");
    assert_eq!(platform::macos_display("shift-tab"), "⇧⇥");
    assert_eq!(platform::macos_display("cmd-space"), "⌘␣");
    assert_eq!(platform::macos_display("cmd-a"), "⌘A");
    // chord 段用空格分隔，不粘连
    assert_eq!(platform::macos_display("ctrl-k ctrl-s"), "⌃K ⌃S");
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
        ("default-macos.json", "cmd-=", "cmd-+", "cmd-="),
        ("default-linux.json", "ctrl-=", "ctrl-+", "ctrl-="),
        ("default-windows.json", "ctrl-=", "ctrl-+", "ctrl-="),
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
