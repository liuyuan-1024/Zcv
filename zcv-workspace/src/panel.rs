//! Panel trait 系统 —— 每个面板是独立 Entity，拥有自己的生命周期、渲染和焦点。
//!
//! 参考 Zed `crates/panel/src/panel.rs` 架构：
//! - `Panel` trait 定义面板接口
//! - `PanelHandle` trait object 抹消具体类型，使 Dock 能统一管理异构面板

use gpui::{AnyView, App, Context, Entity, EventEmitter, FocusHandle, Render, Window};

// ═══ 面板事件 ═════════════════════════════════════════════════════

/// 面板对外发出的事件；宿主在注册面板时订阅并统一处理。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelEvent {
    /// 面板请求关闭（如终端最后一个会话关闭时折叠面板）。
    Close,
    /// 面板自持状态变化（如终端会话增删），宿主应保存布局。
    StateChanged,
}

// ═══ Panel trait ═══════════════════════════════════════════════════

/// 面板核心接口。每个面板是一个独立 Entity<T: Panel>。
pub trait Panel: Render + Sized + EventEmitter<PanelEvent> {
    /// 面板图标 SVG 路径。
    fn icon() -> &'static str;

    /// 面板显示名称（中文）。
    fn label() -> &'static str;

    /// 跨版本持久化使用的稳定标识；不得使用本地化文案或注册顺序。
    fn persistent_name() -> &'static str;

    /// 面板的 FocusHandle。
    fn focus_handle(&self, cx: &App) -> FocusHandle;

    /// 激活/停用回调。
    fn set_active(&mut self, _active: bool, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// 面板自持状态的序列化（如终端的会话列表）；None 表示无状态可持久化。
    fn serialized_state(&self, _cx: &App) -> Option<serde_json::Value> {
        None
    }

    /// 恢复面板自持状态（窗口就绪后经布局快照调用）。
    fn restore_state(
        &mut self,
        _state: serde_json::Value,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }
}

// ═══ PanelHandle trait object ══════════════════════════════════════

/// 抹消具体类型的面板句柄，供 Dock 统一存储和管理异构面板。
pub trait PanelHandle: Send + Sync {
    fn icon(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn persistent_name(&self) -> &'static str;
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App);
    /// 面板自持状态的序列化；None 表示无状态可持久化。
    fn serialized_state(&self, cx: &App) -> Option<serde_json::Value>;
    /// 恢复面板自持状态。
    fn restore_state(&self, state: serde_json::Value, window: &mut Window, cx: &mut App);
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

    fn persistent_name(&self) -> &'static str {
        T::persistent_name()
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.set_active(active, window, cx));
    }

    fn serialized_state(&self, cx: &App) -> Option<serde_json::Value> {
        self.read(cx).serialized_state(cx)
    }

    fn restore_state(&self, state: serde_json::Value, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.restore_state(state, window, cx));
    }

    fn to_any(&self) -> AnyView {
        AnyView::from(self.clone())
    }
}
