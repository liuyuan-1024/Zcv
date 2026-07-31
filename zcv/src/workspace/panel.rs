//! Panel trait 系统 —— 每个面板是独立 Entity，拥有自己的生命周期、渲染和焦点。
//!
//! 参考 Zed `crates/panel/src/panel.rs` 架构：
//! - `Panel` trait 定义面板接口
//! - `PanelHandle` trait object 抹消具体类型，使 Dock 能统一管理异构面板

use gpui::{AnyView, App, Context, Entity, EntityId, FocusHandle, Render, Window, div, prelude::*};

use zcv_theme::color;

// ═══ Panel trait ═══════════════════════════════════════════════════

/// 面板核心接口。每个面板是一个独立 Entity<T: Panel>。
pub(crate) trait Panel: Render + Sized {
    /// 面板唯一标识名，用于持久化和类型查询。
    fn persistent_name() -> &'static str;

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
}

// ═══ PanelHandle trait object ══════════════════════════════════════

/// 抹消具体类型的面板句柄，供 Dock 统一存储和管理异构面板。
pub(crate) trait PanelHandle: Send + Sync {
    fn panel_id(&self) -> EntityId;
    fn persistent_name(&self) -> &'static str;
    fn icon(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn action_name(&self) -> &'static str;
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App);
    /// 返回可渲染的 AnyView。
    fn to_any(&self) -> AnyView;
}

/// 桥接：任何 `Entity<T: Panel>` 自动实现 `PanelHandle`。
impl<T: Panel + 'static> PanelHandle for Entity<T> {
    fn panel_id(&self) -> EntityId {
        self.entity_id()
    }

    fn persistent_name(&self) -> &'static str {
        T::persistent_name()
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

    fn to_any(&self) -> AnyView {
        AnyView::from(self.clone())
    }
}

// ═══ 占位面板 ═════════════════════════════════════════════════════

macro_rules! make_placeholder_panel {
    ($name:ident, $persistent:expr, $icon:expr, $label:expr, $action:expr) => {
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
        }

        impl Render for $name {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .track_focus(&self.focus)
                    .key_context($persistent)
                    .tab_index(0)
                    .text_color(color::current().text_placeholder)
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
    "dock::ToggleVersionControl"
);

make_placeholder_panel!(
    OutlinePanel,
    "Outline",
    "icons/panels/outline.svg",
    "大纲",
    "dock::ToggleOutline"
);

make_placeholder_panel!(
    TerminalPanel,
    "Terminal",
    "icons/panels/terminal.svg",
    "终端",
    "dock::ToggleTerminal"
);

make_placeholder_panel!(
    DebugPanel,
    "Debug",
    "icons/panels/debug.svg",
    "调试",
    "dock::ToggleDebug"
);

make_placeholder_panel!(
    KeyboardShortcutsPanel,
    "KeyboardShortcuts",
    "icons/panels/keyboard_shortcuts.svg",
    "快捷键",
    "dock::ToggleKeyboardShortcuts"
);
