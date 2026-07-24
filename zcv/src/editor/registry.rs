//! View —— 已打开文件及其共享 Buffer。
//!
//! ViewRegistry 只管理文件身份和 Buffer；选区与滚动状态由各 Pane 的 Editor 持有。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{Entity, Global};
use zcv_engine::Buffer;

use crate::workbench::pane_group::ViewId;

/// 一个 View 对应一个 Buffer 的"打开实例"。
pub(crate) struct View {
    pub path: PathBuf,
    pub buffer: Entity<Buffer>,
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
    pub fn register(&mut self, path: PathBuf, buffer: Entity<Buffer>) -> ViewId {
        let id = ViewId(self.next_id);
        self.next_id += 1;
        self.views.insert(id, View { path, buffer });
        id
    }

    pub fn get(&self, id: ViewId) -> Option<&View> {
        self.views.get(&id)
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
