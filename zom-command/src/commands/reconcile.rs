//! "Dispatch 末尾"中央 hook：所有按 scope 自动维护 dismiss 栈的策略集中调用点。
//!
//! 由 [`crate::run`] 在每次 dispatch 结束后调一次（无论成功还是 handler 报错），
//! 把"运行时态 ↔ dismiss 栈"重新对齐：
//! - 比如选区被某个命令扩展了，但 token 没人 push —— 这里补 push；
//! - 反过来选区被塌掉了，但 selection token 还挂在栈顶 —— 这里负责 pop。
//!
//! 每个域的具体策略写在对应 feature 模块里（如 [`super::features::editor::reconcile_text_edit_dismiss`]），
//! 本文件只做编排：新增 scope 时在 [`after_dispatch`] 列表里加一行。

use crate::{CommandContext, EditorEffect, HostEffect, commands::editor};

/// 在 dispatch 末尾对齐所有 dismiss 策略与指针状态。
///
/// 调用约定：必须在所有命令 handler 跑完之后调，不要在 handler 之间调
/// —— 否则 `dismiss_top` 弹出 token 的间隙会被这里再 push 回来，产生死循环。
///
/// 同时清除 pointer selection session —— 任何通过命令系统（键盘、IME、剪贴板等）修改选区的操作都可能让 PointerSelectionSession 里残留的鼠标拖拽锚点失效。
/// 不在这里清除的话，后续 MouseMove 会用旧锚点把命令塌缩好的 caret 重新复活成选区。
pub(crate) fn after_dispatch(context: &mut CommandContext<'_>) {
    editor::reconcile_text_edit_dismiss(context);
    // 新 scope 接入只需要在这里加一行 reconcile 调用——不动 executor、不动 host。

    // 命令系统的任何操作都可能改变选区，使鼠标拖拽会话的锚点失效。
    // cancel_pointer_selection 是 idempotent 的——session 已空时只是 no-op。
    context
        .effects
        .push(HostEffect::Editor(EditorEffect::CancelPointerSelection));
}
