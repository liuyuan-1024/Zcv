//! shell —— GPUI 外壳。
//!
//! 职责（手册 1.2、2.4）：
//! - 视觉层（L1-L4 组件树）
//! - 平台层占位
//! - 启动窗口、装配 `WorkbenchFrame`、提供 `EmbeddedAssetSource`
//!
//! `shell` 不依赖 `app`；本模块只暴露 `run(App)`，由 `main.rs` 调用。

mod boot;
pub(crate) mod features;
pub(crate) mod overlay;
pub(crate) mod platform;
pub(crate) mod shared;
mod view;
pub(crate) mod workbench;

use std::rc::Rc;

use gpui::{App as GpuiApp, Bounds, Pixels, Window};

pub use boot::run;
pub(crate) use shared::keyboard::normalized_chord;

/// 一个已经绑好命令的点击回调 —— shell 子组件不接触 [`Invocation`]，也无需
/// 反向 import `app::*`，触发时直接 `on_click(...)` 即可。
pub(crate) type ActionRequest = Rc<dyn Fn(&mut Window, &mut GpuiApp)>;

/// 顶 bar 的三个圆点共享一份回调包。新加按钮就在这里加字段。
pub(crate) struct WindowControlsHandlers {
    pub(crate) quit: ActionRequest,
    pub(crate) minimize: ActionRequest,
    pub(crate) toggle_maximize: ActionRequest,
}

/// 返回 `true` 表示按键被 keymap 消费（或在等待多段 leader key），调用方应当
/// `stop_propagation`；返回 `false` 表示没有匹配，必须放行给系统输入法。
pub(crate) type KeyRequest = Rc<dyn Fn(String, &mut Window, &mut GpuiApp) -> bool>;
/// 反查某条命令的快捷键文案。`None` 表示命令未绑定，UI 应当不显示快捷键
/// 而非显示空字符串占位。Glyph / 菜单 / 命令面板共用同一份单一真理源。
pub(crate) type ShortcutLookup = Rc<dyn Fn(&str) -> Option<String>>;
/// 在 paint 阶段把活动编辑区的 `Entity<ShellView>` 注册为系统输入法的接收端。
///
/// 由 `ShellView::render` 构造，editor_grid 在 `canvas` paint 回调里调用 ——
/// editor_grid 不持有 `Entity<ShellView>`，只透过这个 hook 接 IME。
pub(crate) type InputHandlerHook = Rc<dyn Fn(Bounds<Pixels>, &mut Window, &mut GpuiApp)>;
