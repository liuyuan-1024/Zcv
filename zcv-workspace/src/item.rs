//! Item 协议：Workspace 标签页中文档视图的通用能力。
//!
//! 只定义 Item 的通用能力，不依赖 Editor、具体预览格式或 Pane 实现。
//! 预览视图等可选能力通过 [`Item::as_preview_item`] 桥接获取（对齐 Zed 的 `as_searchable` 模式），不占用 Item 主接口。

use std::any::TypeId;
use std::path::{Path, PathBuf};

use gpui::{
    AnyEntity, AnyView, App, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    Render, SharedString, Subscription, Task, Window,
};
use zcv_engine::Buffer;
use zcv_project::Project;

use crate::preview::PreviewItemHandle;
use crate::searchable::SearchableItemHandle;
use crate::toolbar::ToolbarItemLocation;

/// Item 向 Pane/Workspace 上报的通用事件，对齐 Zed `workspace::item::ItemEvent`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemEvent {
    /// 标签标题等内容需要刷新。
    UpdateTab,
    /// 面包屑路径需要刷新。
    UpdateBreadcrumbs,
    /// 文档内容被编辑。
    Edit,
}

pub trait Item: Focusable + EventEmitter<Self::Event> + Render + Sized + 'static {
    type Event;

    fn tab_content_text(&self, detail: usize, cx: &App) -> SharedString;

    /// 自定义标签图标（SVG 资源路径）；None 时由 Pane 按文件路径推断。
    fn tab_icon(&self, _cx: &App) -> Option<SharedString> {
        None
    }

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

    /// 预览视图 Item 把自己暴露给 Pane（对齐 Zed 的 `as_searchable` 桥接模式）。
    /// 非预览视图返回 None，无需实现。
    fn as_preview_item(
        &self,
        _self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn PreviewItemHandle>> {
        None
    }

    /// 可搜索 Item 把自己暴露给搜索条（对齐 Zed 的 `as_searchable` 桥接模式）。
    /// 不可搜索的 Item 返回 None，无需实现。
    fn as_searchable(
        &self,
        _self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn SearchableItemHandle>> {
        None
    }

    fn can_save(&self, _cx: &App) -> bool {
        false
    }

    fn save(
        &mut self,
        project: Entity<Project>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let (Some(buffer), Some(path)) = (self.buffer(cx), self.file_path(cx)) else {
            return Task::ready(Ok(()));
        };
        let result = project.update(cx, |project, cx| project.save_buffer(&buffer, &path, cx));
        Task::ready(result.map_err(|error| anyhow::anyhow!("{error}")))
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
    fn tab_content_text(&self, detail: usize, cx: &App) -> SharedString;
    fn tab_icon(&self, cx: &App) -> Option<SharedString>;
    fn is_dirty(&self, cx: &App) -> bool;
    fn file_path(&self, cx: &App) -> Option<PathBuf>;
    fn rename_path(&self, from: &Path, to: &Path, cx: &mut App);
    fn buffer(&self, cx: &App) -> Option<Entity<Buffer>>;
    fn as_preview_item(&self, cx: &App) -> Option<Box<dyn PreviewItemHandle>>;
    fn as_searchable(&self, cx: &App) -> Option<Box<dyn SearchableItemHandle>>;
    fn can_save(&self, cx: &App) -> bool;
    fn save(
        &self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<()>>;
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

    fn tab_content_text(&self, detail: usize, cx: &App) -> SharedString {
        self.read(cx).tab_content_text(detail, cx)
    }

    fn tab_icon(&self, cx: &App) -> Option<SharedString> {
        self.read(cx).tab_icon(cx)
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

    fn as_preview_item(&self, cx: &App) -> Option<Box<dyn PreviewItemHandle>> {
        self.read(cx).as_preview_item(self, cx)
    }

    fn as_searchable(&self, cx: &App) -> Option<Box<dyn SearchableItemHandle>> {
        self.read(cx).as_searchable(self, cx)
    }

    fn can_save(&self, cx: &App) -> bool {
        self.read(cx).can_save(cx)
    }

    fn save(
        &self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<()>> {
        self.update(cx, |item, cx| item.save(project, window, cx))
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
