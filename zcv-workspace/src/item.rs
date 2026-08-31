//! Item 协议：Workspace 标签页中文档视图的通用能力。
//!
//! 只定义 Item 的通用能力，不依赖 Editor、具体预览格式或 Pane 实现。
//! 预览视图等可选能力通过 [`Item::as_preview_item`] 桥接获取，不占用 Item 主接口。

use std::any::TypeId;
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::{
    AnyEntity, AnyView, App, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    Render, SharedString, Subscription, Task, Window,
};
use zcv_multi_buffer::MultiBuffer;
use zcv_project::Project;

use crate::preview::PreviewItemHandle;
use crate::searchable::SearchableItemHandle;
use crate::toolbar::ToolbarItemLocation;

/// Item 向 Pane/Workspace 上报的通用事件。
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

    fn tab_content_text(&self, cx: &App) -> SharedString;

    /// 自定义标签图标（SVG 资源路径）；None 时由 Pane 按文件路径推断。
    fn tab_icon(&self, _cx: &App) -> Option<SharedString> {
        None
    }

    fn to_item_events(_event: &Self::Event, _emit: &mut dyn FnMut(ItemEvent)) {}

    fn is_dirty(&self, _cx: &App) -> bool {
        false
    }

    /// 标签去重、持久化与文件操作使用的稳定身份路径。
    fn item_path(&self, _cx: &App) -> Option<PathBuf> {
        None
    }

    /// 当前光标/视图对应的活动路径。组合文档可与标签身份路径不同。
    fn active_path(&self, cx: &App) -> Option<PathBuf> {
        self.item_path(cx)
    }

    fn breadcrumb_location(&self, _cx: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::Hidden
    }

    fn breadcrumbs(
        &self,
        _project_root: Option<&Path>,
        _cx: &App,
    ) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        None
    }

    fn rename_path(&mut self, _from: &Path, _to: &Path, _cx: &mut Context<Self>) {}

    /// Item 对应的编辑器文档模型；非文本 Item 返回 None。
    fn multi_buffer(&self, _cx: &App) -> Option<Entity<MultiBuffer>> {
        None
    }

    /// 把 Item 定位到 UTF-8 字节范围；不支持文本定位的 Item 返回 false。
    fn navigate_to_byte_range(&mut self, _range: Range<usize>, _cx: &mut Context<Self>) -> bool {
        false
    }

    /// 定位到 0-indexed 逻辑行列；列按 Unicode scalar value 计数。
    fn navigate_to_line_column(
        &mut self,
        _line: usize,
        _column: usize,
        _cx: &mut Context<Self>,
    ) -> bool {
        false
    }

    /// 预览视图 Item 把自己暴露给 Pane。
    /// 非预览视图返回 None，无需实现。
    fn as_preview_item(
        &self,
        _self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn PreviewItemHandle>> {
        None
    }

    /// 可搜索 Item 把自己暴露给搜索条。
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
        _project: Entity<Project>,
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
    fn tab_content_text(&self, cx: &App) -> SharedString;
    fn tab_icon(&self, cx: &App) -> Option<SharedString>;
    fn is_dirty(&self, cx: &App) -> bool;
    fn item_path(&self, cx: &App) -> Option<PathBuf>;
    fn active_path(&self, cx: &App) -> Option<PathBuf>;
    fn rename_path(&self, from: &Path, to: &Path, cx: &mut App);
    fn multi_buffer(&self, cx: &App) -> Option<Entity<MultiBuffer>>;
    fn navigate_to_byte_range(&self, range: Range<usize>, cx: &mut App) -> bool;
    fn navigate_to_line_column(&self, line: usize, column: usize, cx: &mut App) -> bool;
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
    fn breadcrumbs(
        &self,
        project_root: Option<&Path>,
        cx: &App,
    ) -> Option<(Vec<SharedString>, Option<gpui::Font>)>;
}

impl dyn ItemHandle {
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

    fn tab_content_text(&self, cx: &App) -> SharedString {
        self.read(cx).tab_content_text(cx)
    }

    fn tab_icon(&self, cx: &App) -> Option<SharedString> {
        self.read(cx).tab_icon(cx)
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.read(cx).is_dirty(cx)
    }

    fn item_path(&self, cx: &App) -> Option<PathBuf> {
        self.read(cx).item_path(cx)
    }

    fn active_path(&self, cx: &App) -> Option<PathBuf> {
        self.read(cx).active_path(cx)
    }

    fn rename_path(&self, from: &Path, to: &Path, cx: &mut App) {
        self.update(cx, |item, cx| item.rename_path(from, to, cx));
    }

    fn multi_buffer(&self, cx: &App) -> Option<Entity<MultiBuffer>> {
        self.read(cx).multi_buffer(cx)
    }

    fn navigate_to_byte_range(&self, range: Range<usize>, cx: &mut App) -> bool {
        self.update(cx, |item, cx| item.navigate_to_byte_range(range, cx))
    }

    fn navigate_to_line_column(&self, line: usize, column: usize, cx: &mut App) -> bool {
        self.update(cx, |item, cx| {
            item.navigate_to_line_column(line, column, cx)
        })
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

    fn breadcrumbs(
        &self,
        project_root: Option<&Path>,
        cx: &App,
    ) -> Option<(Vec<SharedString>, Option<gpui::Font>)> {
        self.read(cx).breadcrumbs(project_root, cx)
    }
}
