//! Panel trait 系统 —— 每个面板是独立 Entity，拥有自己的生命周期、渲染和焦点。
//!
//! 参考 zed `crates/panel/src/panel.rs` 架构：
//! - `Panel` trait 定义面板接口（所有方法均为静态或 &self）
//! - `PanelHandle` trait object 抹消具体类型，使 Dock 能统一管理异构面板
//! - 空白占位面板类型集中在本文底部

use gpui::{
    AnyView, App, Context, Entity, FocusHandle, Pixels, Render, Window, div, prelude::*, px,
};

use super::dock::DockArea;
use crate::theme::color;

// ═══ Panel trait ═══════════════════════════════════════════════════

/// 面板核心接口。每个面板是一个独立 Entity<T: Panel>。
pub(crate) trait Panel: Render + Sized {
    /// 面板唯一标识名，用于持久化和类型查询。
    fn persistent_name() -> &'static str;

    /// 面板所在的 dock 区域（静态，但允许子类型读 &self 返回固定值）。
    fn position() -> DockArea;

    /// 面板默认尺寸（左/右 dock 为宽度，底 dock 为高度）。
    fn default_size() -> Pixels;

    /// 面板图标 SVG 路径。
    fn icon() -> &'static str;

    /// 面板显示名称（中文）。
    fn label() -> &'static str;

    /// 面板 toggle action 名（如 `"dock::ToggleProjectTree"`），用于快捷键查找。
    fn action_name() -> &'static str;

    /// 面板的 FocusHandle。
    fn focus_handle(&self, cx: &App) -> FocusHandle;

    /// 激活/停用回调。
    fn set_active(&mut self, _active: bool, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// 面板排序优先级（越小越靠前）。
    fn activation_priority() -> u32 {
        0
    }
}

// ═══ PanelHandle trait object ══════════════════════════════════════

/// 抹消具体类型的面板句柄，供 Dock 统一存储和管理异构面板。
pub(crate) trait PanelHandle: Send + Sync {
    fn persistent_name(&self) -> &'static str;
    fn position(&self) -> DockArea;
    fn default_size(&self) -> Pixels;
    fn icon(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn action_name(&self) -> &'static str;
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App);
    fn activation_priority(&self) -> u32;
    /// 返回可渲染的 AnyView。
    fn to_any_view(&self) -> AnyView;
}

/// 桥接：任何 `Entity<T: Panel>` 自动实现 `PanelHandle`。
impl<T: Panel + 'static> PanelHandle for Entity<T> {
    fn persistent_name(&self) -> &'static str {
        T::persistent_name()
    }

    fn position(&self) -> DockArea {
        T::position()
    }

    fn default_size(&self) -> Pixels {
        T::default_size()
    }

    fn icon(&self) -> &'static str {
        T::icon()
    }

    fn label(&self) -> &'static str {
        T::label()
    }

    fn action_name(&self) -> &'static str {
        T::action_name()
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.set_active(active, window, cx));
    }

    fn activation_priority(&self) -> u32 {
        T::activation_priority()
    }

    fn to_any_view(&self) -> AnyView {
        AnyView::from(self.clone())
    }
}

// ═══ 占位面板 ═════════════════════════════════════════════════════

macro_rules! make_placeholder_panel {
    ($name:ident, $persistent:expr, $icon:expr, $label:expr, $action:expr, $area:expr, $size:expr, $priority:expr) => {
        pub(crate) struct $name {
            focus: FocusHandle,
        }

        impl $name {
            pub fn new(cx: &mut Context<Self>) -> Self {
                Self {
                    focus: cx.focus_handle(),
                }
            }
        }

        impl Panel for $name {
            fn persistent_name() -> &'static str {
                $persistent
            }
            fn position() -> DockArea {
                $area
            }
            fn default_size() -> Pixels {
                $size
            }
            fn icon() -> &'static str {
                $icon
            }
            fn label() -> &'static str {
                $label
            }
            fn action_name() -> &'static str {
                $action
            }
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus.clone()
            }
            fn activation_priority() -> u32 {
                $priority
            }
        }

        impl Render for $name {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(color::current().gray.s[5])
                    .child($label)
            }
        }
    };
}

make_placeholder_panel!(
    VersionControlPanel,
    "VersionControl",
    "icons/panels/version_control.svg",
    "版本控制",
    "dock::ToggleVersionControl",
    DockArea::Left,
    px(240.0),
    10
);

make_placeholder_panel!(
    OutlinePanel,
    "Outline",
    "icons/panels/outline.svg",
    "大纲",
    "dock::ToggleOutline",
    DockArea::Left,
    px(240.0),
    20
);

make_placeholder_panel!(
    TerminalPanel,
    "Terminal",
    "icons/panels/terminal.svg",
    "终端",
    "dock::ToggleTerminal",
    DockArea::Bottom,
    px(200.0),
    30
);

make_placeholder_panel!(
    DebugPanel,
    "Debug",
    "icons/panels/debug.svg",
    "调试",
    "dock::ToggleDebug",
    DockArea::Bottom,
    px(200.0),
    40
);

make_placeholder_panel!(
    KeyboardShortcutsPanel,
    "KeyboardShortcuts",
    "icons/panels/keyboard_shortcuts.svg",
    "快捷键",
    "dock::ToggleKeyboardShortcuts",
    DockArea::Right,
    px(240.0),
    50
);
