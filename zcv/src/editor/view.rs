//! View —— Buffer 的一个视口。
//!
//! 每个 View 持有一个 Buffer 引用，记录自己的滚动偏移。
//! 同一 Buffer 可被多个 View 共享（分屏场景）。

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::Global;
use zcv_engine::Buffer;

use crate::workbench::ViewId;

/// 一个 View 对应一个 Buffer 的"打开实例"。
pub(crate) struct View {
    pub id: ViewId,
    pub path: PathBuf,
    pub buffer: Rc<std::cell::RefCell<Buffer>>,
    /// 当前滚动到的行号（0-based）。
    pub scroll_line: Cell<u32>,
}

/// View 全局注册表。
///
/// 存放所有打开的 View，供 layout 渲染时按 ViewId 查找。
pub(crate) struct ViewRegistry {
    views: HashMap<ViewId, View>,
    next_id: u64,
}

impl ViewRegistry {
    pub fn new() -> Self {
        Self {
            views: HashMap::new(),
            next_id: 1,
        }
    }

    /// 注册一个 View，返回分配的 ViewId。
    pub fn register(&mut self, path: PathBuf, buffer: Rc<std::cell::RefCell<Buffer>>) -> ViewId {
        let id = ViewId(self.next_id);
        self.next_id += 1;
        self.views.insert(
            id,
            View {
                id,
                path,
                buffer,
                scroll_line: Cell::new(0),
            },
        );
        id
    }

    pub fn get(&self, id: ViewId) -> Option<&View> {
        self.views.get(&id)
    }

    pub fn get_mut(&mut self, id: ViewId) -> Option<&mut View> {
        self.views.get_mut(&id)
    }

    /// 按文件路径查找已有 View。
    pub fn find_by_path(&self, path: &Path) -> Option<ViewId> {
        self.views
            .iter()
            .find_map(|(id, v)| if v.path == path { Some(*id) } else { None })
    }

    pub fn remove(&mut self, id: ViewId) {
        self.views.remove(&id);
    }
}

impl Global for ViewRegistry {}
