//! ItemHandle —— Pane 中标签页的通用抽象。
//!
//! 定义 [`ItemHandle`] trait，让 Pane 可持有任意视图类型（编辑器、欢迎页等），不再硬编码 `Entity<Editor>`。

use std::any::Any;
use std::path::PathBuf;

use gpui::{
    AnyElement, App, Entity, EntityId, FocusHandle, IntoElement, SharedString, Subscription, Window,
};

use super::toolbar::ToolbarItemLocation;
use crate::editor::Editor;

// ═══ ItemEvent ═══════════════════════════════════════════════════════

/// Item 对外发出的事件，供 Toolbar 子项等订阅。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ItemEvent {
    /// 面包屑文本发生变化，需要刷新。
    UpdateBreadcrumbs,
    /// 脏状态发生变化（用于标签指示器）。
    DirtyChanged,
}

// ═══ ItemHandle trait ════════════════════════════════════════════════

/// Pane 中单个标签页的对象安全句柄。
///
/// Pane 通过此 trait 统一操作所有标签页，不关心具体类型。
pub(crate) trait ItemHandle: Send + 'static {
    /// 标签显示标题。
    fn title(&self, cx: &App) -> SharedString;
    /// 是否有未保存修改。
    fn is_dirty(&self, cx: &App) -> bool;
    /// 关联文件路径（如果有）。
    fn file_path(&self, cx: &App) -> Option<PathBuf>;
    /// 焦点句柄。
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    /// 实体 ID。
    fn entity_id(&self) -> EntityId;
    /// 转为 GPUI 元素用于渲染。
    fn to_any_element(&self) -> AnyElement;
    /// 向下转型为 `Any`，供需要具体类型的组件使用。
    fn as_any(&self) -> &dyn Any;

    // ── Toolbar / 面包屑 ──────────────────────────────────────────

    /// 克隆自身为 trait object。
    fn boxed_clone(&self) -> Box<dyn ItemHandle>;

    /// 订阅 item 发出的 [`ItemEvent`]。
    fn subscribe_to_item_events(
        &self,
        _window: &mut Window,
        cx: &mut App,
        callback: Box<dyn Fn(&ItemEvent, &mut App) + Send>,
    ) -> Subscription;

    /// 面包屑在 Toolbar 中的位置。
    fn breadcrumb_location(&self, cx: &App) -> ToolbarItemLocation;

    /// 面包屑文本段及字体。
    ///
    /// `Vec<SharedString>` 是路径各分段（如 `["project", "src", "main.rs"]`），
    /// `Option<Font>` 是可选字体。
    fn breadcrumbs(&self, cx: &App) -> Option<(Vec<SharedString>, Option<gpui::Font>)>;

    /// 面包屑前缀元素（文件图标等）。
    fn breadcrumb_prefix(&self, _window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
        None
    }
}

// ═══ Entity<Editor> 实现 ItemHandle ════════════════════════════════

impl ItemHandle for Entity<Editor> {
    fn title(&self, cx: &App) -> SharedString {
        self.read(cx)
            .file_path()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
            .into()
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.read(cx).is_dirty(cx)
    }

    fn file_path(&self, cx: &App) -> Option<PathBuf> {
        self.read(cx).file_path().map(|p| p.to_path_buf())
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle()
    }

    fn entity_id(&self) -> EntityId {
        self.entity_id()
    }

    fn to_any_element(&self) -> AnyElement {
        self.clone().into_any_element()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn boxed_clone(&self) -> Box<dyn ItemHandle> {
        Box::new(self.clone())
    }

    fn subscribe_to_item_events(
        &self,
        _window: &mut Window,
        cx: &mut App,
        callback: Box<dyn Fn(&ItemEvent, &mut App) + Send>,
    ) -> Subscription {
        let entity = self.clone();
        self.update(cx, |_this, cx| {
            cx.subscribe::<Editor, ItemEvent>(&entity, move |_, _, event, cx| {
                callback(event, cx);
            })
        })
    }

    fn breadcrumb_location(&self, _cx: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::PrimaryLeft
    }

    fn breadcrumbs(&self, cx: &App) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        let path = self.read(cx).file_path()?;
        // 相对于项目根显示，从项目根开始剥掉前缀
        let relative = cx
            .try_global::<super::ProjectRoot>()
            .and_then(|root| path.strip_prefix(&root.0).ok())
            .unwrap_or(path);
        let segments: Vec<SharedString> = relative
            .iter()
            .map(|component| component.to_string_lossy().into_owned().into())
            .collect();
        Some((segments, None))
    }
}
