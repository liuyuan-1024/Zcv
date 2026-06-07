//! `settings.*` 命令目录。
//!
//! 设置界面暂未实现；命令先完整注册，宿主收到 effect 后决定展示占位或忽略。
//!
//! esc 走系统级 [`crate::commands::system::dismiss::DISMISS_TOP`]（scope=Settings）—— [`OPEN`] 推一条 dismiss token，esc 弹出后重新派发 [`DISMISS`]。

use crate::commands::system::dismiss as dismiss_top;
use crate::{
    CommandArgs, CommandContext, CommandError, CommandId, CommandOutcome, CommandRegistry,
    DismissScope, HostEffect, Invocation, KeyBindingContext, Keymap, NoArgs,
};

/// 打开设置面板。
pub const OPEN: &str = "settings.open";
/// 关闭设置面板。
pub const DISMISS: &str = "settings.dismiss";

/// 设置面板拥有自己的键盘上下文，Esc 等面板内按键不污染全局快捷键空间。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsKeyContext;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsBindingContext;

pub fn open() -> Invocation {
    (cid(OPEN), CommandArgs::new())
}

pub fn dismiss() -> Invocation {
    (cid(DISMISS), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let settings = KeyBindingContext::settings();

    registry
        .install(keymap, OPEN, "设置", Box::new(run_open))
        .description("打开设置面板。")
        .key("mod-,");

    registry.install(keymap, DISMISS, "关闭设置", Box::new(run_dismiss));

    dismiss_top::bind_esc(keymap, DismissScope::Settings, settings);
}

fn run_open(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::Settings);
    context
        .dismiss
        .push(DismissScope::Settings, "关闭设置", dismiss());
    context.effects.push(HostEffect::ShowSettings);
    Ok(CommandOutcome::default())
}

fn run_dismiss(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::Settings);
    context.effects.push(HostEffect::DismissSurface);
    Ok(CommandOutcome::default())
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
