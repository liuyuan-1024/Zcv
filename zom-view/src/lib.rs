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

use zom_engine::{BufferVersion, ByteOffset, FoldSet, SelectionSet};
use zom_workspace::BufferId;

/// view 的标识。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ViewId(u64);

impl ViewId {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// 滚动位置 / 可见区域。
///
/// `top_line` 是当前视口顶部可见的逻辑行（0-based）；`visible_line_count` 是
/// 视口能容纳的整行数。两者一起决定 `Buffer::slice_viewport` 切出哪一段。
///
/// 这两个值由渲染端（`zom-desktop` 的 `EditorElement` prepaint）按 bounds /
/// line_height 反算后写回 View —— 编辑面状态层只持值，不计算像素。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ViewportState {
    /// 顶部可见的逻辑行（0-based）。
    pub top_line: u64,
    /// 视口能容纳的整行数；为 0 表示尚未测量。
    pub visible_line_count: u64,
}

/// 「请把某个 byte 滚到视区」的意图标签。
///
/// 调用方表达「为什么要 reveal」，由渲染端（editor element）翻译成具体
/// 的滚动策略（位置 / 是否仅在不可见时触发 / 是否伴随高亮等）。这层抽象
/// 让 zom-view 不沾"上 1/3"这种渲染概念，调整全局风格只动渲染端的 mapping。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevealKind {
    /// 搜索 / 迭代型导航。尊重用户当前视区：目标已经可见就不动，不可见才滚（渲染端通常摆到上 1/3）。
    /// 配合后续高亮 active match 使用 —— 用户能从高亮看到选中变化，视区无需每次跳。
    Match,
    /// 主动跳转（goto-definition / goto-line / 跳诊断 等）。
    /// 用户在另一个控件输入命令后明确要求"把我带到那里"，无论目标是否在视区都给明显的视觉反馈。
    Jump,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevealRequest {
    pub byte: ByteOffset,
    pub kind: RevealKind,
    /// 单调递增计数。同一个 byte + 同一个 kind 也能被反复触发：
    /// 渲染端按seq 判别「是不是新请求」。
    pub seq: u64,
}

/// 一个视图：对某个 buffer 的一次观察。
#[derive(Debug)]
pub struct View {
    buffer: BufferId,
    selection: SelectionSet,
    folds: FoldSet,
    viewport: ViewportState,
    reveal: Option<RevealRequest>,
    reveal_seq: u64,
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
            reveal: None,
            reveal_seq: 0,
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

    pub fn reveal(&self) -> Option<RevealRequest> {
        self.reveal
    }

    /// 请求把 `byte` 滚到视区。`kind` 标明触发场景；具体怎么滚由渲染端解读。
    /// 每次调用都推进 seq —— 哪怕同 byte / 同 kind 也算「新一次请求」，
    /// 让渲染端有依据再触发一次。
    pub fn request_reveal(&mut self, byte: ByteOffset, kind: RevealKind) {
        self.reveal_seq = self.reveal_seq.wrapping_add(1);
        self.reveal = Some(RevealRequest {
            byte,
            kind,
            seq: self.reveal_seq,
        });
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
