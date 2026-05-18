//! `editor.save` 命令目录。
//!
//! 落在 `zom-command` 而非 `zom-desktop` —— handler 只用 `Workspace::save_file`，
//! 这是 zom-command 已有的依赖，没有 GPUI / shell / 扩展服务参与。
//! 放在这里可以无头测试，与其它 `editor.*` 命令同口径。

use crate::{
    CommandArgs, CommandContext, CommandError, CommandId, CommandOutcome, CommandRegistry,
    Invocation, Keymap, NoArgs,
};
use zom_workspace::BufferId;

pub const SAVE: &str = "editor.save";

/// 打开工作区选择器（切换工作区入口）。**尚未实现** —— 命令未注册，
/// 占住 id 给 top_bar workspace 标签引用，避免裸字符串。
pub const SHOW_PICKER: &str = "workspace.show_picker";

/// 用于命令面板 / 菜单等以编程方式触发保存。键盘绑 `mod-s` 走 keymap 直派发，
/// 不经此 builder。
#[allow(dead_code)]
pub fn save() -> Invocation {
    (
        CommandId::new(SAVE).expect("内建命令 ID 必须非空"),
        CommandArgs::new(),
    )
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    registry
        .install(
            keymap,
            SAVE,
            "保存",
            Box::new(|ctx, args| {
                NoArgs::try_from(args)?;
                let buffer_id = active_view_buffer_id(ctx)?;
                ctx.workspace
                    .save_file(buffer_id)
                    .map_err(|error| CommandError::ExecutionFailed(error.to_string()))?;
                Ok(CommandOutcome::default())
            }),
        )
        .key("mod-s");
}

fn active_view_buffer_id(ctx: &CommandContext<'_>) -> Result<BufferId, CommandError> {
    ctx.views
        .active_view()
        .map(|view| view.buffer())
        .ok_or(CommandError::NoActiveView)
}
