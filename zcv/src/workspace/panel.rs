//! Panel trait 系统 —— 每个面板是独立 Entity，拥有自己的生命周期、渲染和焦点。
//!
//! 参考 Zed `crates/panel/src/panel.rs` 架构：
//! - `Panel` trait 定义面板接口
//! - `PanelHandle` trait object 抹消具体类型，使 Dock 能统一管理异构面板

use gpui::{Action, AnyView, App, Context, Entity, FocusHandle, Render, Window, div, prelude::*};

use zcv_theme::color;

// ═══ Panel trait ═══════════════════════════════════════════════════

/// 面板核心接口。每个面板是一个独立 Entity<T: Panel>。
///
/// toggle action 由面板类型自身声明，快捷键、底栏按钮与 Workspace 分派都从它派生，不再维护字符串映射表。
pub(crate) trait Panel: Render + Sized {
    /// 该面板的 toggle action 类型。
    type ToggleAction: Action + Default;

    /// 面板图标 SVG 路径。
    fn icon() -> &'static str;

    /// 面板显示名称（中文）。
    fn label() -> &'static str;

    /// 面板的 FocusHandle。
    fn focus_handle(&self, cx: &App) -> FocusHandle;

    /// 激活/停用回调。
    fn set_active(&mut self, _active: bool, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// 面板 toggle action 实例，默认由 `Self::ToggleAction` 构造。
    fn toggle_action(&self, _cx: &App) -> Box<dyn Action> {
        Box::new(Self::ToggleAction::default())
    }
}

// ═══ PanelHandle trait object ══════════════════════════════════════

/// 抹消具体类型的面板句柄，供 Dock 统一存储和管理异构面板。
pub(crate) trait PanelHandle: Send + Sync {
    fn icon(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn toggle_action(&self, cx: &App) -> Box<dyn Action>;
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App);
    /// 返回可渲染的 AnyView。
    fn to_any(&self) -> AnyView;
}

/// 桥接：任何 `Entity<T: Panel>` 自动实现 `PanelHandle`。
impl<T: Panel + 'static> PanelHandle for Entity<T> {
    fn icon(&self) -> &'static str {
        T::icon()
    }

    fn label(&self) -> &'static str {
        T::label()
    }

    fn toggle_action(&self, cx: &App) -> Box<dyn Action> {
        self.read(cx).toggle_action(cx)
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.set_active(active, window, cx));
    }

    fn to_any(&self) -> AnyView {
        AnyView::from(self.clone())
    }
}

// ═══ 占位面板 ═════════════════════════════════════════════════════

macro_rules! make_placeholder_panel {
    ($name:ident, $toggle_action:ty, $persistent:expr, $icon:expr, $label:expr) => {
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
            type ToggleAction = $toggle_action;

            fn icon() -> &'static str {
                $icon
            }
            fn label() -> &'static str {
                $label
            }
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus.clone()
            }
        }

        impl Render for $name {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .track_focus(&self.focus)
                    .key_context($persistent)
                    .tab_index(0)
                    .text_color(color::current(cx).text_placeholder)
                    .child($label)
            }
        }
    };
}

make_placeholder_panel!(
    OutlinePanel,
    super::dock::ToggleOutline,
    "Outline",
    "icons/list_tree.svg",
    "大纲"
);

make_placeholder_panel!(
    TerminalPanel,
    super::dock::ToggleTerminal,
    "Terminal",
    "icons/terminal.svg",
    "终端"
);

make_placeholder_panel!(
    DebugPanel,
    super::dock::ToggleDebug,
    "Debug",
    "icons/debug.svg",
    "调试"
);

make_placeholder_panel!(
    KeyboardShortcutsPanel,
    super::dock::ToggleKeyboardShortcuts,
    "KeyboardShortcuts",
    "icons/keyboard.svg",
    "快捷键"
);
