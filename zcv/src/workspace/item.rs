//! Item 与 ItemHandle —— Pane 中标签页的强类型能力和类型擦除句柄。
//!
//! 具体视图实现 [`Item`]，`Entity<T>` 通过统一桥接获得 [`ItemHandle`]，Pane 因而可以持有编辑器、欢迎页等异构视图。

use std::path::PathBuf;

use gpui::{
    AnyView, App, Entity, EntityId, EventEmitter, FocusHandle, Focusable, Render, SharedString,
    Subscription, Window,
};

use super::toolbar::ToolbarItemLocation;
use zcv_editor::{Editor, EditorEvent};

// ═══ ItemEvent ═══════════════════════════════════════════════════════

/// Item 对外发出的事件，供 Toolbar 子项等订阅。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ItemEvent {
    /// 面包屑文本发生变化，需要刷新。
    UpdateBreadcrumbs,
}

// ═══ Item trait ══════════════════════════════════════════════════════

/// Pane 中具体标签页视图的强类型能力。
pub(crate) trait Item:
    Focusable + EventEmitter<Self::Event> + Render + Sized + 'static
{
    type Event;

    /// 标签显示标题。
    fn tab_content_text(&self, cx: &App) -> SharedString;

    /// 把具体视图事件映射为 Workspace 可理解的通用 ItemEvent。
    fn to_item_events(_event: &Self::Event, _emit: &mut dyn FnMut(ItemEvent)) {}

    /// 是否有未保存修改。
    fn is_dirty(&self, _cx: &App) -> bool {
        false
    }

    /// 关联文件路径（如果有）。
    fn file_path(&self, _cx: &App) -> Option<PathBuf> {
        None
    }

    /// 面包屑在 Toolbar 中的位置。
    fn breadcrumb_location(&self, _cx: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::Hidden
    }

    /// 面包屑文本段及字体。
    fn breadcrumbs(&self, _cx: &App) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        None
    }
}

// ═══ ItemHandle trait ════════════════════════════════════════════════

/// [`Item`] 的对象安全句柄，供 Pane 统一存储异构视图。
pub(crate) trait ItemHandle: Send + 'static {
    /// 实体 ID。
    fn item_id(&self) -> EntityId;
    /// 焦点句柄。
    fn item_focus_handle(&self, cx: &App) -> FocusHandle;
    /// 转为 GPUI 视图，保留 Entity 身份和向下转型能力。
    fn to_any_view(&self) -> AnyView;
    /// 克隆自身为 trait object。
    fn boxed_clone(&self) -> Box<dyn ItemHandle>;
    /// 标签显示标题。
    fn tab_content_text(&self, cx: &App) -> SharedString;
    /// 是否有未保存修改。
    fn is_dirty(&self, cx: &App) -> bool;
    /// 关联文件路径（如果有）。
    fn file_path(&self, cx: &App) -> Option<PathBuf>;

    /// 订阅 item 发出的 [`ItemEvent`]。
    fn subscribe_to_item_events(
        &self,
        _window: &mut Window,
        cx: &mut App,
        handler: Box<dyn Fn(ItemEvent, &mut App) + Send>,
    ) -> Subscription;

    /// 面包屑在 Toolbar 中的位置。
    fn breadcrumb_location(&self, cx: &App) -> ToolbarItemLocation;

    /// 面包屑文本段及字体。
    ///
    /// `Vec<SharedString>` 是路径各分段（如 `["project", "src", "main.rs"]`），`Option<Font>` 是可选字体。
    fn breadcrumbs(&self, cx: &App) -> Option<(Vec<SharedString>, Option<gpui::Font>)>;
}

impl dyn ItemHandle {
    pub(crate) fn downcast<T: Render + 'static>(&self) -> Option<Entity<T>> {
        self.to_any_view().downcast().ok()
    }
}

impl Clone for Box<dyn ItemHandle> {
    fn clone(&self) -> Self {
        self.boxed_clone()
    }
}

/// 桥接：任何 `Entity<T: Item>` 自动获得对象安全的 ItemHandle。
impl<T: Item> ItemHandle for Entity<T> {
    fn item_id(&self) -> EntityId {
        self.entity_id()
    }

    fn item_focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn to_any_view(&self) -> AnyView {
        self.clone().into()
    }

    fn boxed_clone(&self) -> Box<dyn ItemHandle> {
        Box::new(self.clone())
    }

    fn tab_content_text(&self, cx: &App) -> SharedString {
        self.read(cx).tab_content_text(cx)
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.read(cx).is_dirty(cx)
    }

    fn file_path(&self, cx: &App) -> Option<PathBuf> {
        self.read(cx).file_path(cx)
    }

    fn subscribe_to_item_events(
        &self,
        _window: &mut Window,
        cx: &mut App,
        handler: Box<dyn Fn(ItemEvent, &mut App) + Send>,
    ) -> Subscription {
        let entity = self.clone();
        self.update(cx, |_item, cx| {
            cx.subscribe::<T, T::Event>(&entity, move |_, _, event, cx| {
                T::to_item_events(event, &mut |event| handler(event, cx));
            })
        })
    }

    fn breadcrumb_location(&self, cx: &App) -> ToolbarItemLocation {
        self.read(cx).breadcrumb_location(cx)
    }

    fn breadcrumbs(&self, cx: &App) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        self.read(cx).breadcrumbs(cx)
    }
}

// ═══ Editor Item 实现 ═══════════════════════════════════════════════

impl Item for Editor {
    type Event = EditorEvent;

    fn tab_content_text(&self, cx: &App) -> SharedString {
        self.file_path(cx)
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .unwrap_or_default()
            .into()
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        match event {
            EditorEvent::PathChanged => emit(ItemEvent::UpdateBreadcrumbs),
        }
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.is_dirty(cx)
    }

    fn file_path(&self, cx: &App) -> Option<PathBuf> {
        self.file_path(cx)
    }

    fn breadcrumb_location(&self, _cx: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::PrimaryLeft
    }

    fn breadcrumbs(&self, cx: &App) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        let path = self.file_path(cx)?;
        let relative = self
            .project_root()
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(&path);
        Some((vec![relative.to_string_lossy().into_owned().into()], None))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, TestAppContext, div, prelude::*};

    use super::*;

    struct TestItem {
        focus: FocusHandle,
    }

    impl EventEmitter<()> for TestItem {}

    impl Focusable for TestItem {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Render for TestItem {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().child("测试 Item")
        }
    }

    impl Item for TestItem {
        type Event = ();

        fn tab_content_text(&self, _cx: &App) -> SharedString {
            "测试".into()
        }
    }

    #[gpui::test]
    fn entity_item_uses_generic_item_handle_bridge(cx: &mut TestAppContext) {
        let item = cx.new(|cx| TestItem {
            focus: cx.focus_handle(),
        });
        let handle: Box<dyn ItemHandle> = Box::new(item.clone());

        cx.read(|cx| {
            assert_eq!(handle.item_id(), item.entity_id());
            assert_eq!(handle.tab_content_text(cx).as_ref(), "测试");
            assert!(handle.downcast::<TestItem>().is_some());
        });
    }
}
