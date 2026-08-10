//! Item 协议：Workspace 标签页中文档视图的通用能力。
//!
//! 只定义 Item 的通用能力，不依赖 Editor、具体预览格式或 Pane 实现。

use std::any::{Any, TypeId};
use std::path::{Path, PathBuf};

use gpui::{
    AnyEntity, AnyView, App, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    Render, SharedString, Subscription, Task, Window,
};
use zcv_engine::Buffer;
use zcv_project::Project;

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

/// Item 向 Pane/Workspace 上报的通用事件，对齐 Zed `workspace::item::ItemEvent`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemEvent {
    /// Item 请求关闭自身标签页。
    CloseItem,
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

    fn tab_tooltip_text(&self, _cx: &App) -> Option<SharedString> {
        None
    }

    fn to_item_events(_event: &Self::Event, _emit: &mut dyn FnMut(ItemEvent)) {}

    /// 标签页从激活变为非激活。
    fn deactivated(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// 标签页从 Pane 移除时调用（关闭前）。
    fn on_removed(&self, _cx: &mut Context<Self>) {}

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

    /// 预览视图返回其对应的源码 Item；非预览视图返回 None。
    fn source_item(&self, _cx: &App) -> Option<Box<dyn ItemHandle>> {
        None
    }

    /// 编辑后是否保持预览状态；false 时 Pane 在文档编辑后提升为固定标签。
    fn preserve_preview(&self, _cx: &App) -> bool {
        false
    }

    fn can_save(&self, _cx: &App) -> bool {
        false
    }

    fn can_save_as(&self, _cx: &App) -> bool {
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

    fn save_as(
        &mut self,
        _project: Entity<Project>,
        _path: PathBuf,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        Task::ready(Ok(()))
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
    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString>;
    fn is_dirty(&self, cx: &App) -> bool;
    fn file_path(&self, cx: &App) -> Option<PathBuf>;
    fn rename_path(&self, from: &Path, to: &Path, cx: &mut App);
    fn buffer(&self, cx: &App) -> Option<Entity<Buffer>>;
    fn document_item_key(&self, cx: &App) -> Option<DocumentItemKey>;
    fn source_item(&self, cx: &App) -> Option<Box<dyn ItemHandle>>;
    fn preserve_preview(&self, cx: &App) -> bool;
    fn can_save(&self, cx: &App) -> bool;
    fn can_save_as(&self, cx: &App) -> bool;
    fn save(
        &self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<()>>;
    fn save_as(
        &self,
        project: Entity<Project>,
        path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<()>>;
    fn deactivated(&self, window: &mut Window, cx: &mut App);
    fn on_removed(&self, cx: &mut App);
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

    fn tab_content_text(&self, detail: usize, cx: &App) -> SharedString {
        self.read(cx).tab_content_text(detail, cx)
    }

    fn tab_icon(&self, cx: &App) -> Option<SharedString> {
        self.read(cx).tab_icon(cx)
    }

    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString> {
        self.read(cx).tab_tooltip_text(cx)
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

    fn source_item(&self, cx: &App) -> Option<Box<dyn ItemHandle>> {
        self.read(cx).source_item(cx)
    }

    fn preserve_preview(&self, cx: &App) -> bool {
        self.read(cx).preserve_preview(cx)
    }

    fn can_save(&self, cx: &App) -> bool {
        self.read(cx).can_save(cx)
    }

    fn can_save_as(&self, cx: &App) -> bool {
        self.read(cx).can_save_as(cx)
    }

    fn save(
        &self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<()>> {
        self.update(cx, |item, cx| item.save(project, window, cx))
    }

    fn save_as(
        &self,
        project: Entity<Project>,
        path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<()>> {
        self.update(cx, |item, cx| item.save_as(project, path, window, cx))
    }

    fn deactivated(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |item, cx| item.deactivated(window, cx));
    }

    fn on_removed(&self, cx: &mut App) {
        self.update(cx, |item, cx| item.on_removed(cx));
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
