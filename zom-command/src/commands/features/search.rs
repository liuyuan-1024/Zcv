//! 搜索 feature 命令。

use crate::commands::emit;
use crate::{
    CommandArgs, CommandId, CommandRegistry, HostEffect, Invocation, KeyBindingContext, Keymap,
    SearchOption,
};

pub const TOGGLE_PANEL: &str = "panel.toggle.search";
/// 打开搜索面板并聚焦 query 输入框；面板已聚焦则收起。
///
/// 第一版只承载单文件搜索（per-buffer），不再分 in_buffer / in_project —— 跨文件搜索是 workspace 层的另一个东西，后续单开命令。
pub const ACTIVATE: &str = "search.activate";
pub const TOGGLE_CASE_SENSITIVE: &str = "search.toggle_case_sensitive";
pub const TOGGLE_WHOLE_WORD: &str = "search.toggle_whole_word";
pub const TOGGLE_REGEX: &str = "search.toggle_regex";
pub const FIND_PREVIOUS: &str = "search.find_previous";
pub const FIND_NEXT: &str = "search.find_next";
pub const REPLACE_NEXT: &str = "search.replace_next";
pub const REPLACE_ALL: &str = "search.replace_all";
pub const FOCUS_NEXT_FIELD: &str = "search.focus_next_field";
pub const FOCUS_PREVIOUS_FIELD: &str = "search.focus_previous_field";
pub const FOCUS_EDITOR: &str = "search.focus_editor";

pub fn toggle_panel() -> Invocation {
    super::panel_toggle_invocation(TOGGLE_PANEL)
}

pub fn activate() -> Invocation {
    no_args(ACTIVATE)
}

pub fn toggle_case_sensitive() -> Invocation {
    no_args(TOGGLE_CASE_SENSITIVE)
}

pub fn toggle_whole_word() -> Invocation {
    no_args(TOGGLE_WHOLE_WORD)
}

pub fn toggle_regex() -> Invocation {
    no_args(TOGGLE_REGEX)
}

pub fn find_previous() -> Invocation {
    no_args(FIND_PREVIOUS)
}

pub fn find_next() -> Invocation {
    no_args(FIND_NEXT)
}

pub fn replace_next() -> Invocation {
    no_args(REPLACE_NEXT)
}

pub fn replace_all() -> Invocation {
    no_args(REPLACE_ALL)
}

pub fn focus_next_field() -> Invocation {
    no_args(FOCUS_NEXT_FIELD)
}

pub fn focus_previous_field() -> Invocation {
    no_args(FOCUS_PREVIOUS_FIELD)
}

pub fn focus_editor() -> Invocation {
    no_args(FOCUS_EDITOR)
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let search_panel = KeyBindingContext::search_panel();

    registry.install(
        keymap,
        TOGGLE_PANEL,
        "搜索",
        emit(HostEffect::TogglePanel("search".to_string())),
    );
    registry
        .install(keymap, ACTIVATE, "搜索", emit(HostEffect::SearchActivate))
        .description("打开搜索面板并聚焦 query；已聚焦则收起。第一版只搜当前 buffer。")
        .key("mod-f");
    registry
        .install(
            keymap,
            TOGGLE_CASE_SENSITIVE,
            "区分大小写",
            emit(HostEffect::SearchToggleOption(SearchOption::CaseSensitive)),
        )
        .key_in("alt-c", search_panel);
    registry
        .install(
            keymap,
            TOGGLE_WHOLE_WORD,
            "全词匹配",
            emit(HostEffect::SearchToggleOption(SearchOption::WholeWord)),
        )
        .key_in("alt-w", search_panel);
    registry
        .install(
            keymap,
            TOGGLE_REGEX,
            "正则表达式",
            emit(HostEffect::SearchToggleOption(SearchOption::Regex)),
        )
        .key_in("alt-r", search_panel);
    registry
        .install(
            keymap,
            FIND_PREVIOUS,
            "上一个",
            emit(HostEffect::SearchFindPrevious),
        )
        .key_in("up", search_panel);
    registry
        .install(
            keymap,
            FIND_NEXT,
            "下一个",
            emit(HostEffect::SearchFindNext),
        )
        .key_in("down", search_panel);
    registry
        .install(
            keymap,
            REPLACE_NEXT,
            "替换下一个",
            emit(HostEffect::SearchReplaceNext),
        )
        .key_in("mod-enter", search_panel);
    registry
        .install(
            keymap,
            REPLACE_ALL,
            "全部替换",
            emit(HostEffect::SearchReplaceAll),
        )
        .key_in("mod-shift-enter", search_panel);
    registry
        .install(
            keymap,
            FOCUS_NEXT_FIELD,
            "聚焦下一个搜索输入框",
            emit(HostEffect::SearchFocusNextField),
        )
        .key_in("tab", search_panel);
    registry
        .install(
            keymap,
            FOCUS_PREVIOUS_FIELD,
            "聚焦上一个搜索输入框",
            emit(HostEffect::SearchFocusPreviousField),
        )
        .key_in("shift-tab", search_panel);
    registry
        .install(
            keymap,
            FOCUS_EDITOR,
            "焦点回到当前编辑器",
            emit(HostEffect::SearchFocusEditor),
        )
        .key_in("escape", search_panel)
        .key_in("enter", search_panel);
}

fn no_args(command_id: &'static str) -> Invocation {
    (cid(command_id), CommandArgs::new())
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
