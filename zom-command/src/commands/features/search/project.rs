//! 项目级（跨文件）搜索命令。
//!
//! 当前为 placeholder：宿主收到 [`HostEffect::ShowBubble`] 后弹一条
//! "敬请期待"气泡。完整实现接入 workspace 层的多文件命中索引后会替换为
//! 一个独立的 `HostEffect::SearchProjectActivate`，与本文件平级。

use crate::commands::emit;
use crate::{
    BubbleRequest, CommandArgs, CommandId, CommandRegistry, HostEffect, Invocation, Keymap,
};

/// 项目级搜索入口。当前 placeholder：宿主弹一条"敬请期待"气泡。
pub const PROJECT_ACTIVATE: &str = "search.project_activate";

pub fn project_activate() -> Invocation {
    no_args(PROJECT_ACTIVATE)
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    registry
        .install(
            keymap,
            PROJECT_ACTIVATE,
            "项目搜索",
            emit(HostEffect::ShowBubble(
                BubbleRequest::info("项目级搜索敬请期待").dedupe("search.project_activate"),
            )),
        )
        .description("跨文件搜索入口（待实现）。")
        .key("mod-shift-f");
}

fn no_args(command_id: &'static str) -> Invocation {
    (cid(command_id), CommandArgs::new())
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
