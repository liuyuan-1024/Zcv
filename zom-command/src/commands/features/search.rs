//! 搜索 feature 命令。

use crate::{
    CommandArgs, CommandContext, CommandError, CommandId, CommandOutcome, CommandRegistry,
    HostEffect, Invocation, KeyBindingContext, Keymap, NoArgs, SearchOption, SearchScope,
};

pub const TOGGLE_PANEL: &str = "panel.toggle.search";
pub const SCOPE_CURRENT_FILE: &str = "search.scope_current_file";
pub const SCOPE_PROJECT: &str = "search.scope_project";
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

pub fn scope_current_file() -> Invocation {
    no_args(SCOPE_CURRENT_FILE)
}

pub fn scope_project() -> Invocation {
    no_args(SCOPE_PROJECT)
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

    super::register_panel_toggle(
        registry,
        keymap,
        TOGGLE_PANEL,
        "search",
        "搜索",
        "打开搜索面板，在当前文件或整个项目中查找文本。",
        "mod-shift-f",
    );
    registry
        .install(
            keymap,
            SCOPE_CURRENT_FILE,
            "当前文件",
            Box::new(run_scope_current_file),
        )
        .description("将搜索范围切换为当前文件。")
        .key("mod-b");
    registry
        .install(
            keymap,
            SCOPE_PROJECT,
            "整个项目",
            Box::new(run_scope_project),
        )
        .description("将搜索范围切换为整个项目。")
        .key("mod-p");
    registry
        .install(
            keymap,
            TOGGLE_CASE_SENSITIVE,
            "区分大小写",
            Box::new(run_toggle_case_sensitive),
        )
        .description("搜索时区分字母大小写。")
        .key("alt-c");
    registry
        .install(
            keymap,
            TOGGLE_WHOLE_WORD,
            "全词匹配",
            Box::new(run_toggle_whole_word),
        )
        .description("只匹配完整单词。")
        .key("alt-w");
    registry
        .install(
            keymap,
            TOGGLE_REGEX,
            "正则表达式",
            Box::new(run_toggle_regex),
        )
        .description("使用正则表达式进行搜索。")
        .key("alt-r");
    registry
        .install(keymap, FIND_PREVIOUS, "上一个", Box::new(run_find_previous))
        .description("跳到上一个搜索结果。")
        .key_in("up", search_panel);
    registry
        .install(keymap, FIND_NEXT, "下一个", Box::new(run_find_next))
        .description("跳到下一个搜索结果。")
        .key_in("down", search_panel);
    registry
        .install(
            keymap,
            REPLACE_NEXT,
            "替换下一个",
            Box::new(run_replace_next),
        )
        .description("替换当前匹配项并跳到下一个结果。")
        .key("mod-enter");
    registry
        .install(keymap, REPLACE_ALL, "全部替换", Box::new(run_replace_all))
        .description("替换当前范围内的所有匹配项。")
        .key("mod-shift-enter");
    registry
        .install(
            keymap,
            FOCUS_NEXT_FIELD,
            "聚焦下一个搜索输入框",
            Box::new(run_focus_next_field),
        )
        .description("在搜索输入框之间向后移动焦点。")
        .key_in("tab", search_panel);
    registry
        .install(
            keymap,
            FOCUS_PREVIOUS_FIELD,
            "聚焦上一个搜索输入框",
            Box::new(run_focus_previous_field),
        )
        .description("在搜索输入框之间向前移动焦点。")
        .key_in("shift-tab", search_panel);
    registry
        .install(
            keymap,
            FOCUS_EDITOR,
            "焦点回到当前编辑器",
            Box::new(run_focus_editor),
        )
        .description("让焦点从搜索面板回到当前编辑器。")
        .key_in("escape", search_panel)
        .key_in("enter", search_panel);
}

fn no_args(command_id: &'static str) -> Invocation {
    (cid(command_id), CommandArgs::new())
}

fn run_scope_current_file(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    ctx.effects
        .push(HostEffect::SearchSetScope(SearchScope::CurrentFile));
    Ok(CommandOutcome::default())
}

fn run_scope_project(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    ctx.effects
        .push(HostEffect::SearchSetScope(SearchScope::Project));
    Ok(CommandOutcome::default())
}

fn run_toggle_case_sensitive(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    ctx.effects
        .push(HostEffect::SearchToggleOption(SearchOption::CaseSensitive));
    Ok(CommandOutcome::default())
}

fn run_toggle_whole_word(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    ctx.effects
        .push(HostEffect::SearchToggleOption(SearchOption::WholeWord));
    Ok(CommandOutcome::default())
}

fn run_toggle_regex(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    ctx.effects
        .push(HostEffect::SearchToggleOption(SearchOption::Regex));
    Ok(CommandOutcome::default())
}

fn run_find_previous(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    ctx.effects.push(HostEffect::SearchFindPrevious);
    Ok(CommandOutcome::default())
}

fn run_find_next(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    ctx.effects.push(HostEffect::SearchFindNext);
    Ok(CommandOutcome::default())
}

fn run_replace_next(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    ctx.effects.push(HostEffect::SearchReplaceNext);
    Ok(CommandOutcome::default())
}

fn run_replace_all(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    ctx.effects.push(HostEffect::SearchReplaceAll);
    Ok(CommandOutcome::default())
}

fn run_focus_next_field(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    ctx.effects.push(HostEffect::SearchFocusNextField);
    Ok(CommandOutcome::default())
}

fn run_focus_previous_field(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    ctx.effects.push(HostEffect::SearchFocusPreviousField);
    Ok(CommandOutcome::default())
}

fn run_focus_editor(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    ctx.effects.push(HostEffect::SearchFocusEditor);
    Ok(CommandOutcome::default())
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
