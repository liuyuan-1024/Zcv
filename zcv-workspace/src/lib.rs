//! Workspace 标签页协议。
//!
//! 该 crate 只定义 Item 的通用能力，不依赖 Editor、具体预览格式或 Pane 实现。

use std::any::{Any, TypeId};
use std::path::{Path, PathBuf};

use gpui::{
    AnyEntity, AnyView, App, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    Render, SharedString, Subscription,
};
use zcv_engine::Buffer;

/// Toolbar 子项的布局位置。
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ToolbarItemLocation {
    Hidden,
    PrimaryLeft,
    PrimaryRight,
}

/// 标签当前展示的文档表现。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemPresentation {
    Source,
    Preview(&'static str),
}

/// 文档及其展示形态的稳定键。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentItemKey {
    pub buffer_id: EntityId,
    pub presentation: ItemPresentation,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemEvent {
    UpdateBreadcrumbs,
}

pub trait Item: Focusable + EventEmitter<Self::Event> + Render + Sized + 'static {
    type Event;

    fn tab_content_text(&self, cx: &App) -> SharedString;

    fn to_item_events(_event: &Self::Event, _emit: &mut dyn FnMut(ItemEvent)) {}

    fn is_dirty(&self, _cx: &App) -> bool {
        false
    }

    fn file_path(&self, _cx: &App) -> Option<PathBuf> {
        None
    }

    fn breadcrumb_location(&self, _cx: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::Hidden
    }

    fn breadcrumbs(&self, _cx: &App) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        None
    }

    fn rename_path(&mut self, _from: &Path, _to: &Path, _cx: &mut Context<Self>) {}

    fn buffer(&self, _cx: &App) -> Option<Entity<Buffer>> {
        None
    }

    fn document_item_key(&self, _cx: &App) -> Option<DocumentItemKey> {
        None
    }

    fn act_as_type(
        &self,
        type_id: TypeId,
        self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<AnyEntity> {
        (TypeId::of::<Self>() == type_id).then(|| self_handle.clone().into())
    }
}

pub type ItemEventHandler = Box<dyn Fn(ItemEvent, &mut App) + Send>;

pub trait ItemHandle: Send + 'static {
    fn item_id(&self) -> EntityId;
    fn item_focus_handle(&self, cx: &App) -> FocusHandle;
    fn to_any_view(&self) -> AnyView;
    fn boxed_clone(&self) -> Box<dyn ItemHandle>;
    fn tab_content_text(&self, cx: &App) -> SharedString;
    fn is_dirty(&self, cx: &App) -> bool;
    fn file_path(&self, cx: &App) -> Option<PathBuf>;
    fn rename_path(&self, from: &Path, to: &Path, cx: &mut App);
    fn buffer(&self, cx: &App) -> Option<Entity<Buffer>>;
    fn document_item_key(&self, cx: &App) -> Option<DocumentItemKey>;
    fn act_as_type(&self, type_id: TypeId, cx: &App) -> Option<AnyEntity>;
    fn subscribe_to_item_events(&self, cx: &mut App, handler: ItemEventHandler) -> Subscription;
    fn breadcrumb_location(&self, cx: &App) -> ToolbarItemLocation;
    fn breadcrumbs(&self, cx: &App) -> Option<(Vec<SharedString>, Option<gpui::Font>)>;
}

impl dyn ItemHandle {
    pub fn downcast<T: Render + 'static>(&self) -> Option<Entity<T>> {
        self.to_any_view().downcast().ok()
    }

    pub fn act_as<T: 'static>(&self, cx: &App) -> Option<Entity<T>> {
        self.act_as_type(TypeId::of::<T>(), cx)?.downcast().ok()
    }

    pub fn is<T: Any>(&self, cx: &App) -> bool {
        self.act_as_type(TypeId::of::<T>(), cx).is_some()
    }
}

impl Clone for Box<dyn ItemHandle> {
    fn clone(&self) -> Self {
        self.boxed_clone()
    }
}

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

    fn rename_path(&self, from: &Path, to: &Path, cx: &mut App) {
        self.update(cx, |item, cx| item.rename_path(from, to, cx));
    }

    fn buffer(&self, cx: &App) -> Option<Entity<Buffer>> {
        self.read(cx).buffer(cx)
    }

    fn document_item_key(&self, cx: &App) -> Option<DocumentItemKey> {
        self.read(cx).document_item_key(cx)
    }

    fn act_as_type(&self, type_id: TypeId, cx: &App) -> Option<AnyEntity> {
        self.read(cx).act_as_type(type_id, self, cx)
    }

    fn subscribe_to_item_events(&self, cx: &mut App, handler: ItemEventHandler) -> Subscription {
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

#[cfg(test)]
mod tests {
    use gpui::{AppContext, Context, IntoElement, ParentElement, TestAppContext, Window, div};

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
