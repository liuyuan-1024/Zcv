//! 多个 shell 功能共享的交互回调原语。

use std::rc::Rc;

use gpui::{App as GpuiApp, Bounds, Pixels, Window};

/// 一个已经绑好命令的点击回调。UI 子组件不接触命令对象，触发时直接调用。
pub(crate) type ActionRequest = Rc<dyn Fn(&mut Window, &mut GpuiApp)>;

/// 顶 bar 的三个圆点共享一份回调包。新加按钮就在这里加字段。
pub(crate) struct WindowControlsHandlers {
    pub(crate) quit: ActionRequest,
    pub(crate) minimize: ActionRequest,
    pub(crate) toggle_maximize: ActionRequest,
}

/// 返回 `true` 表示按键被 keymap 消费，调用方应当停止传播；返回 `false`
/// 表示没有匹配，必须放行给系统输入法。
pub(crate) type KeyRequest = Rc<dyn Fn(String, &mut Window, &mut GpuiApp) -> bool>;

/// 反查某条命令的快捷键文案。`None` 表示命令未绑定，UI 不显示快捷键占位。
pub(crate) type ShortcutLookup = Rc<dyn Fn(&str) -> Option<String>>;

/// 在 paint 阶段把活动编辑区的 `Entity<ShellView>` 注册为系统输入法接收端。
pub(crate) type InputHandlerHook = Rc<dyn Fn(Bounds<Pixels>, &mut Window, &mut GpuiApp)>;
