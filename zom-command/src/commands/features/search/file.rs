//! 文件级（per-buffer）搜索 / 替换命令。
//!
//! 由 `mod-f` 唤起编辑器上方的内联 bar：query / replacement 双输入框 + 命中高亮 + 上下导航。
//! 算法层落在 `WorkspaceBuffer::BufferSearch`，命令侧只 emit `HostEffect`，宿主翻译。

use crate::commands::emit;
use crate::{
    CommandArgs, CommandId, CommandRegistry, HostEffect, Invocation, KeyBindingContext, Keymap,
    SearchOption,
};

/// 打开当前文件的内联搜索栏并聚焦 query 输入框。已开则只搬焦点（幂等）。
/// 收起走 `escape`（[`FOCUS_EDITOR`]），不在本命令里复用。
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
/// Esc 路径：把光标折叠到当前命中末尾，再收起 bar 并把焦点交回编辑器。
pub const FOCUS_EDITOR: &str = "search.focus_editor";
/// Enter 路径：把光标折叠到当前命中末尾、焦点回编辑器；**bar 保留**。
/// 想再改 query，从编辑器按 mod-f 即可回到 query 输入框。
pub const CONFIRM_MATCH: &str = "search.confirm_match";

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

pub fn confirm_match() -> Invocation {
    no_args(CONFIRM_MATCH)
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let text_edit = KeyBindingContext::text_edit();
    let search_input = KeyBindingContext::search_input();

    registry
        .install(keymap, ACTIVATE, "查找", emit(HostEffect::SearchActivate))
        .description("打开当前文件的内联搜索栏并聚焦 query。收起请按 Esc。")
        .key_in("mod-f", text_edit);
    registry
        .install(
            keymap,
            TOGGLE_CASE_SENSITIVE,
            "区分大小写",
            emit(HostEffect::SearchToggleOption(SearchOption::CaseSensitive)),
        )
        .key_in("alt-c", search_input);
    registry
        .install(
            keymap,
            TOGGLE_WHOLE_WORD,
            "全词匹配",
            emit(HostEffect::SearchToggleOption(SearchOption::WholeWord)),
        )
        .key_in("alt-w", search_input);
    registry
        .install(
            keymap,
            TOGGLE_REGEX,
            "正则表达式",
            emit(HostEffect::SearchToggleOption(SearchOption::Regex)),
        )
        .key_in("alt-r", search_input);
    registry
        .install(
            keymap,
            FIND_PREVIOUS,
            "上一个",
            emit(HostEffect::SearchFindPrevious),
        )
        .key_in("up", search_input);
    registry
        .install(
            keymap,
            FIND_NEXT,
            "下一个",
            emit(HostEffect::SearchFindNext),
        )
        .key_in("down", search_input);
    registry
        .install(
            keymap,
            REPLACE_NEXT,
            "替换下一个",
            emit(HostEffect::SearchReplaceNext),
        )
        .key_in("mod-enter", search_input);
    registry
        .install(
            keymap,
            REPLACE_ALL,
            "全部替换",
            emit(HostEffect::SearchReplaceAll),
        )
        .key_in("mod-shift-enter", search_input);
    registry
        .install(
            keymap,
            FOCUS_NEXT_FIELD,
            "聚焦下一个搜索输入框",
            emit(HostEffect::SearchFocusNextField),
        )
        .key_in("tab", search_input);
    registry
        .install(
            keymap,
            FOCUS_PREVIOUS_FIELD,
            "聚焦上一个搜索输入框",
            emit(HostEffect::SearchFocusPreviousField),
        )
        .key_in("shift-tab", search_input);
    registry
        .install(
            keymap,
            FOCUS_EDITOR,
            "退出搜索",
            emit(HostEffect::SearchFocusEditor),
        )
        .description("取消搜索栏，把光标折叠到当前命中末尾。")
        .key_in("escape", search_input);
    registry
        .install(
            keymap,
            CONFIRM_MATCH,
            "跳转到匹配末尾",
            emit(HostEffect::SearchConfirmMatch),
        )
        .description("把光标折叠到当前命中末尾")
        .key_in("enter", search_input);
}

fn no_args(command_id: &'static str) -> Invocation {
    (cid(command_id), CommandArgs::new())
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
