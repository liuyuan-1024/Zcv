//! `editor.save` 命令目录。
//!
//! 落在 `zom-command` 而非 `zom-desktop` —— handler 只用 `Workspace::save_file`，
//! 这是 zom-command 已有的依赖，没有 GPUI / shell / 扩展服务参与。
//! 放在这里可以无头测试，与其它 `editor.*` 命令同口径。

use crate::{
    CommandArgs, CommandContext, CommandError, CommandHandler, CommandId, CommandOutcome,
    CommandRegistry, HostEffect, Invocation, Keymap, NoArgs,
};
use zom_workspace::BufferId;

pub const SAVE: &str = "editor.save";

/// 打开项目选择器（顶栏"切换项目"入口）。
///
/// 注：模块名保留 `workspace` 作为内部代号；面向用户文案统一用"项目"。
pub const SHOW_PROJECTS_PICKER: &str = "workspace.show_projects_picker";

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

    registry
        .install(
            keymap,
            SHOW_PROJECTS_PICKER,
            "切换项目",
            emit(HostEffect::ShowProjectPicker),
        )
        .key("mod-o");
}

/// 与 `window.rs::emit` 同形态；catalog 里"按一个键就推一个 effect"的样板。
fn emit(effect: HostEffect) -> CommandHandler {
    Box::new(move |ctx, args| {
        NoArgs::try_from(args)?;
        ctx.effects.push(effect.clone());
        Ok(CommandOutcome::default())
    })
}

fn active_view_buffer_id(ctx: &CommandContext<'_>) -> Result<BufferId, CommandError> {
    ctx.views
        .active_view()
        .map(|view| view.buffer())
        .ok_or(CommandError::NoActiveView)
}
