//! ItemHandle —— Pane 中标签页的通用抽象。
//!
//! 定义 [`ItemHandle`] trait，让 Pane 可持有任意视图类型（编辑器、欢迎页等），不再硬编码 `Entity<Editor>`。

use std::any::Any;
use std::path::PathBuf;

use gpui::{AnyElement, App, Entity, EntityId, FocusHandle, IntoElement, SharedString};

use crate::editor::editor::Editor;

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
}

/// `Entity<Editor>` 实现 ItemHandle。
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
}
