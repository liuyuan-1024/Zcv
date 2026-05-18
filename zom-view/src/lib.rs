//! zom-view —— 编辑面状态层
//!
//! 持有「我怎么看一个 buffer」的状态：看哪个 buffer、滚到哪、本视图的
//! 光标与折叠。判据：同一文件开两个分屏*会*不同的状态归这里（光标、
//! fold、滚动）；属于文件本身的归 `zom-workspace`。
//!
//! `SelectionSet` / `FoldSet` 的*实例*归 view —— engine 只提供类型和
//! 移动 / after-edit 算法，实例由宿主按视图持有。
//!
//! 骨架阶段：类型形状已定，selection movement 已可由 `zom-command` 经活动
//! view 接入；viewport slice 与 fold / projection 的宿主侧接入留待
//! `TODO.md` P3。

use std::collections::BTreeMap;

use zom_engine::{BufferVersion, FoldSet, SelectionSet};
use zom_workspace::BufferId;

/// view 的标识。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ViewId(u64);

impl ViewId {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// 滚动位置 / 可见区域。骨架阶段先留最小形态，P3 接 viewport slice 时展开。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ViewportState {
    /// 顶部可见的逻辑行。
    pub top_line: u64,
}

/// 一个视图：对某个 buffer 的一次观察。
#[derive(Debug)]
pub struct View {
    buffer: BufferId,
    selection: SelectionSet,
    folds: FoldSet,
    viewport: ViewportState,
}

impl View {
    /// 新建视图。`base_version` 是被观察 buffer 的当前版本 —— `FoldSet`
    /// 必须版本绑定，因此构造时必须提供。
    pub fn new(buffer: BufferId, base_version: BufferVersion) -> Self {
        Self {
            buffer,
            selection: SelectionSet::default(),
            folds: FoldSet::new(base_version),
            viewport: ViewportState::default(),
        }
    }

    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    pub fn selection(&self) -> &SelectionSet {
        &self.selection
    }

    pub fn selection_mut(&mut self) -> &mut SelectionSet {
        &mut self.selection
    }

    pub fn folds(&self) -> &FoldSet {
        &self.folds
    }

    pub fn folds_mut(&mut self) -> &mut FoldSet {
        &mut self.folds
    }

    pub fn viewport(&self) -> ViewportState {
        self.viewport
    }

    pub fn set_viewport(&mut self, viewport: ViewportState) {
        self.viewport = viewport;
    }
}

/// 全部 view 的集合，并记录当前活动 view。
#[derive(Debug, Default)]
pub struct ViewSet {
    next_view_id: u64,
    views: BTreeMap<ViewId, View>,
    active: Option<ViewId>,
}

impl ViewSet {
    pub fn new() -> Self {
        Self {
            next_view_id: 1,
            views: BTreeMap::new(),
            active: None,
        }
    }

    /// 为某个 buffer 新建视图；若当前没有活动视图，则设为活动。
    pub fn open_view(&mut self, buffer: BufferId, base_version: BufferVersion) -> ViewId {
        let id = ViewId(self.next_view_id);
        self.next_view_id += 1;
        self.views.insert(id, View::new(buffer, base_version));
        if self.active.is_none() {
            self.active = Some(id);
        }
        id
    }

    /// 关闭视图；若关掉的是活动视图，则把活动指向任意剩余视图。
    pub fn close_view(&mut self, id: ViewId) {
        self.views.remove(&id);
        if self.active == Some(id) {
            self.active = self.views.keys().next().copied();
        }
    }

    pub fn active(&self) -> Option<ViewId> {
        self.active
    }

    pub fn set_active(&mut self, id: ViewId) {
        if self.views.contains_key(&id) {
            self.active = Some(id);
        }
    }

    pub fn active_view(&self) -> Option<&View> {
        self.active.and_then(|id| self.views.get(&id))
    }

    pub fn active_view_mut(&mut self) -> Option<&mut View> {
        match self.active {
            Some(id) => self.views.get_mut(&id),
            None => None,
        }
    }

    pub fn view(&self, id: ViewId) -> Option<&View> {
        self.views.get(&id)
    }

    pub fn view_mut(&mut self, id: ViewId) -> Option<&mut View> {
        self.views.get_mut(&id)
    }

    pub fn views(&self) -> impl Iterator<Item = (ViewId, &View)> {
        self.views.iter().map(|(id, view)| (*id, view))
    }
}
