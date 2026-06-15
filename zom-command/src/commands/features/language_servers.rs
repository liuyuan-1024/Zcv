//! `language_server.*` 命令目录。
//!
//! 语言服务器域只表达"当前项目打开了哪些语言服务器、状态如何"；诊断问题列表留在 `diagnostics.*` 域。
//!
//! esc 走系统级 [`crate::commands::system::dismiss::DISMISS_TOP`]（scope=LanguageServers）—— [`OPEN`] 推一条 dismiss token，esc 弹出后重新派发 [`DISMISS`]。

use crate::commands::system::dismiss as dismiss_top;
use crate::{
    CommandArgs, CommandContext, CommandError, CommandId, CommandOutcome, CommandRegistry,
    DismissScope, HostEffect, Invocation, KeyBindingContext, Keymap, NoArgs,
};

pub const OPEN: &str = "language_server.open";
pub const DISMISS: &str = "language_server.dismiss";

/// 语言服务器浮面拥有自己的键盘上下文，Esc 等面板内按键不污染全局快捷键空间。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LanguageServersKeyContext;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LanguageServersBindingContext;

pub fn open() -> Invocation {
    (cid(OPEN), CommandArgs::new())
}

pub fn dismiss() -> Invocation {
    (cid(DISMISS), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let language_servers = KeyBindingContext::language_servers();

    registry
        .install(keymap, OPEN, "语言服务器", Box::new(run_open))
        .description("查看当前项目的语言服务器状态。")
        .key("mod shift l");

    registry.install(keymap, DISMISS, "关闭语言服务器", Box::new(run_dismiss));

    dismiss_top::bind_esc(keymap, DismissScope::LanguageServers, language_servers);
}

fn run_open(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::LanguageServers);
    context
        .dismiss
        .push(DismissScope::LanguageServers, "关闭语言服务器", dismiss());
    context.effects.push(HostEffect::ShowLanguageServers);
    Ok(CommandOutcome::default())
}

fn run_dismiss(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::LanguageServers);
    context.effects.push(HostEffect::DismissSurface);
    Ok(CommandOutcome::default())
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
