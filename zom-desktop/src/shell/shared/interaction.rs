//! 多个 shell 功能共享的交互回调原语与命令查表类型。

use std::rc::Rc;

use gpui::{App as GpuiApp, FocusHandle, Window};
use zom_command::CommandCatalogItem;

/// 设备连续交互请求。例如 resize、scroll、selection drag——不适合进入 command catalog 的高频操作。
/// 组件只提交领域内交互事件，真正的状态修改仍由 shell / feature runtime 持有的 handler 完成。
pub(crate) type InteractionRequest<Event> = Rc<dyn Fn(Event, &mut Window, &mut GpuiApp)>;

/// 焦点类交互请求的目标。
#[derive(Clone)]
pub(crate) enum FocusRequestTarget {
    Handle(FocusHandle),
}

/// shell 预绑定的焦点交互出口。
pub(crate) type FocusRequest = InteractionRequest<FocusRequestTarget>;

pub(crate) fn focus_request() -> FocusRequest {
    Rc::new(|target, window, _cx| match target {
        FocusRequestTarget::Handle(handle) => window.focus(&handle),
    })
}

/// 反查某条命令的快捷键文案。`None` 表示命令未绑定，UI 不显示快捷键占位。
pub(crate) type ShortcutLookup = Rc<dyn Fn(&str) -> Option<String>>;

/// 反查某条命令的显示标题。占位命令尚未注册时由调用方提供领域内兜底文案。
pub(crate) type CommandTitleLookup = Rc<dyn Fn(&str) -> Option<String>>;

/// 读取当前命令系统的可展示元数据；具体过滤和排版由面板自己完成。
pub(crate) type CommandCatalogLookup = Rc<dyn Fn() -> Vec<CommandCatalogItem>>;
