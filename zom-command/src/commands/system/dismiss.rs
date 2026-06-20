//! `system.dismiss_top` —— 全部 scope 共用的 esc 路由命令。
//!
//! 设计意图：避免"每个 scope 都注册一份长得一样、只有 scope 不同的 `<scope>.dismiss_top`"。
//! 这条命令接受 [`DismissScope`] 作为参数；每个上下文家族在自己的 `install` 里只用
//! [`bind_esc`] 给 escape 绑一条预填 scope 的快捷键，不再声明任何 per-scope 命令 id。
//!
//! 命令执行体只做两件事：
//! 1. 从 `ctx.dismiss` 弹出指定 scope 的栈顶 token；
//! 2. 把 token 携带的 [`crate::Invocation`] 重新 `dispatch` 进命令队列。
//!
//! 栈空时 no-op：本帧没有可取消的瞬态，esc 静默不消耗。

use crate::{
    CommandArgs, CommandContext, CommandError, CommandOutcome, CommandRegistry, DismissScope,
    KeyBinding, KeyBindingContext, KeyChord, Keymap, reject_unknown_args, required_arg,
};

pub const DISMISS_TOP: &str = "system.dismiss_top";

const ARG_SCOPE: &str = "scope";

/// `system.dismiss_top` 的类型化参数：要弹哪个 scope 的栈顶。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DismissTopArgs {
    pub scope: DismissScope,
}

impl From<DismissTopArgs> for CommandArgs {
    fn from(args: DismissTopArgs) -> Self {
        CommandArgs::new().with(ARG_SCOPE, args.scope.as_str())
    }
}

impl TryFrom<CommandArgs> for DismissTopArgs {
    type Error = CommandError;

    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &[ARG_SCOPE])?;
        let raw = required_arg(&args, ARG_SCOPE)?;
        let scope: DismissScope = raw
            .parse()
            .map_err(|_| CommandError::InvalidArgs(format!("未知 dismiss scope：{raw}")))?;
        Ok(Self { scope })
    }
}

pub fn install(registry: &mut CommandRegistry, _keymap: &mut Keymap) {
    // 单独注册命令；不在这里绑默认键 —— 每个 scope 在自己的 install 里用 [`bind_esc`]
    // 绑到对应的 [`KeyBindingContext`]，预填 scope 参数。
    registry
        .install(_keymap, DISMISS_TOP, "弹出栈顶 dismiss", Box::new(run))
        .hide_from_shortcuts();
}

/// 给某个上下文绑 esc → `system.dismiss_top`(scope=...)。
/// feature 模块的 install 函数调用本助手，不需要直接碰 [`KeyBinding`]。
pub fn bind_esc(keymap: &mut Keymap, scope: DismissScope, context: KeyBindingContext) {
    bind(keymap, "escape", scope, context);
}

/// 绑任意 chord。esc 是惯例入口，但留个口子方便测试 / 异端配置。
pub fn bind(
    keymap: &mut Keymap,
    chord: &'static str,
    scope: DismissScope,
    context: KeyBindingContext,
) {
    let chord = KeyChord::new(chord).expect("快捷键必须非空");
    keymap.bind(KeyBinding {
        sequence: vec![chord],
        command: crate::commands::cid(DISMISS_TOP),
        args: DismissTopArgs { scope }.into(),
        context,
    });
}

fn run(ctx: &mut CommandContext<'_>, args: CommandArgs) -> Result<CommandOutcome, CommandError> {
    let DismissTopArgs { scope } = DismissTopArgs::try_from(args)?;
    if let Some((id, args)) = ctx.dismiss.pop_top(scope) {
        ctx.queue.enqueue(id, args);
    }
    Ok(CommandOutcome::default())
}
