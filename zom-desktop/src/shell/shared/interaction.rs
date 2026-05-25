//! 多个 shell 功能共享的交互回调原语。

use std::rc::Rc;

use gpui::{App as GpuiApp, Window};

/// 一个已经绑好命令的点击回调。UI 子组件不接触命令对象，触发时直接调用。
pub(crate) type ActionRequest = Rc<dyn Fn(&mut Window, &mut GpuiApp)>;

/// 返回 `true` 表示按键被 keymap 消费，调用方应当停止传播；返回 `false`
/// 表示没有匹配，必须放行给系统输入法。
pub(crate) type KeyRequest = Rc<dyn Fn(String, &mut Window, &mut GpuiApp) -> bool>;

/// 反查某条命令的快捷键文案。`None` 表示命令未绑定，UI 不显示快捷键占位。
pub(crate) type ShortcutLookup = Rc<dyn Fn(&str) -> Option<String>>;

/// 反查某条命令的显示标题。占位命令尚未注册时由调用方提供领域内 fallback。
pub(crate) type CommandTitleLookup = Rc<dyn Fn(&str) -> Option<String>>;

/// 命令系统暴露给快捷键面板的只读命令元数据。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommandCatalogItem {
    pub(crate) command_id: String,
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) visible_in_shortcuts: bool,
}

/// 读取当前命令系统的可展示元数据；具体过滤和排版由面板自己完成。
pub(crate) type CommandCatalogLookup = Rc<dyn Fn() -> Vec<CommandCatalogItem>>;
