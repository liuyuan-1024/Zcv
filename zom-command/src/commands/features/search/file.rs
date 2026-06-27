//! 文件级（per-buffer）搜索 / 替换命令。
//!
//! 算法层落在 `WorkspaceBuffer::BufferSearch`，命令侧只 emit `HostEffect`，宿主翻译。
//!
//! esc 走系统级 [`crate::commands::system::dismiss::DISMISS_TOP`] 弹出后重新派发 [`DISMISS`]。

use crate::commands::cid;
use crate::commands::emit;
use crate::commands::system::dismiss as dismiss_top;
use crate::{
    CommandArgs, CommandContext, CommandError, CommandOutcome, CommandRegistry, DismissScope,
    HostEffect, Invocation, KeyBindingContext, Keymap, NoArgs, SearchOption,
};

pub const TOGGLE_CASE_SENSITIVE: &str = "search.toggle_case_sensitive";
pub const TOGGLE_WHOLE_WORD: &str = "search.toggle_whole_word";
pub const TOGGLE_REGEX: &str = "search.toggle_regex";
pub const FIND_PREVIOUS: &str = "search.find_previous";
pub const FIND_NEXT: &str = "search.find_next";
pub const REPLACE_NEXT: &str = "search.replace_next";
pub const REPLACE_ALL: &str = "search.replace_all";
pub const FOCUS_NEXT_FIELD: &str = "search.focus_next_field";
pub const FOCUS_PREVIOUS_FIELD: &str = "search.focus_previous_field";
/// Esc 路径：收起搜索栏，焦点交还给上一个焦点位置。
pub const DISMISS: &str = "search.dismiss";
/// 切换搜索栏开关。
pub const TOGGLE: &str = "search.toggle";
/// Enter 路径：把光标折叠到当前命中末尾、焦点回编辑器；**搜索栏保留**。
pub const CONFIRM_MATCH: &str = "search.confirm_match";

pub fn toggle() -> Invocation {
    no_args(TOGGLE)
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

pub fn dismiss() -> Invocation {
    no_args(DISMISS)
}

pub fn confirm_match() -> Invocation {
    no_args(CONFIRM_MATCH)
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let text_edit = KeyBindingContext::text_edit();
    let search_input = KeyBindingContext::search_input();

    registry
        .install(
            keymap,
            TOGGLE_CASE_SENSITIVE,
            "区分大小写",
            emit(HostEffect::SearchToggleOption(SearchOption::CaseSensitive)),
        )
        .key_in("alt c", search_input);
    registry
        .install(
            keymap,
            TOGGLE_WHOLE_WORD,
            "全词匹配",
            emit(HostEffect::SearchToggleOption(SearchOption::WholeWord)),
        )
        .key_in("alt w", search_input);
    registry
        .install(
            keymap,
            TOGGLE_REGEX,
            "正则表达式",
            emit(HostEffect::SearchToggleOption(SearchOption::Regex)),
        )
        .key_in("alt r", search_input);
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
        .key_in("mod enter", search_input);
    registry
        .install(
            keymap,
            REPLACE_ALL,
            "全部替换",
            emit(HostEffect::SearchReplaceAll),
        )
        .key_in("mod shift enter", search_input);
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
        .key_in("shift tab", search_input);
    registry.install(keymap, DISMISS, "退出搜索", Box::new(run_dismiss));
    registry
        .install(keymap, TOGGLE, "查找", Box::new(run_toggle))
        .key_in("mod f", text_edit);
    registry
        .install(
            keymap,
            CONFIRM_MATCH,
            "跳转到匹配末尾",
            Box::new(run_confirm_match),
        )
        .description("把光标折叠到当前命中末尾")
        .key_in("enter", search_input);

    dismiss_top::bind_esc(keymap, DismissScope::SearchInput, search_input);
}

fn run_toggle(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::SearchInput);
    context
        .dismiss
        .push(DismissScope::SearchInput, "退出搜索", dismiss());
    context.effects.push(HostEffect::SearchToggle);
    Ok(CommandOutcome::default())
}

fn run_dismiss(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::SearchInput);
    context.effects.push(HostEffect::SearchDismiss);
    Ok(CommandOutcome::default())
}

fn run_confirm_match(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::SearchInput);
    context.effects.push(HostEffect::SearchConfirmMatch);
    Ok(CommandOutcome::default())
}

fn no_args(command_id: &'static str) -> Invocation {
    (cid(command_id), CommandArgs::new())
}
