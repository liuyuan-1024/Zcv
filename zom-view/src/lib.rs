//! 编辑面状态层。
//!
//! 持有「我怎么看一个缓冲区（buffer）」的状态。
//! 包括看哪个缓冲区、滚到哪、本视图的光标与折叠。
//! 判据：同一文件开两个分屏会不同的状态归这里。
//! 属于文件本身的状态归 `zom-workspace`。
//!
//! `SelectionSet` / `FoldSet` 的实例归视图层持有。
//! `zom-engine` 提供类型、光标移动与编辑后状态转移算法。

use std::collections::BTreeMap;

use zom_engine::{BufferVersion, ByteOffset, FoldSet, SelectionSet};
use zom_workspace::BufferId;

/// 视图标识。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ViewId(u64);

impl ViewId {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// 视口尚未由渲染端测量过时，`View::new` 给 `visible_line_count` 的初始估值。
///
/// 取一个接近常见大屏首帧需求、又不会让小缓冲区过度分配的值。
/// 第一帧渲染元素测出真实值后会同步回写覆盖。
pub const DEFAULT_INITIAL_VISIBLE_LINES: u64 = 200;

/// 滚动位置 / 可见区域。
///
/// `top_line` 是当前视口顶部可见的逻辑行（0-based）。
/// `visible_line_count` 是视口能容纳的整行数。
/// 两者一起决定 `Buffer::slice_viewport` 切出哪一段。
///
/// `top_line` 的真源是视图自身，由 `settle_viewport_y` 落定。
/// `visible_line_count` 由渲染端按边界和行高测量后同步回写。
/// `View::new` 给 `visible_line_count` 非零初值。
/// 消费侧不需要再为「还没测过」保留特殊路径。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewportState {
    /// 顶部可见的逻辑行（0-based）。
    pub top_line: u64,
    /// 视口能容纳的整行数。
    pub visible_line_count: u64,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            top_line: 0,
            visible_line_count: DEFAULT_INITIAL_VISIBLE_LINES,
        }
    }
}

/// 「请把某个字节位置滚到视区」的意图标签。
///
/// 调用方表达为什么要显露目标位置。
/// 渲染端再翻译成具体滚动策略：位置、是否仅在不可见时触发、是否伴随高亮等。
/// 这层抽象让 `zom-view` 不绑定具体视觉风格。
/// 调整全局风格时只需要改渲染端映射。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevealKind {
    /// 搜索或迭代型导航。尊重用户当前视区：目标已经可见就不动，不可见才滚。
    /// 配合当前匹配项高亮使用，用户能从高亮看到选中变化，视区无需每次跳。
    Match,
    /// 主动跳转，如跳到定义、跳到行、跳到诊断。
    /// 用户明确要求「把我带到那里」，无论目标是否在视区都给明显视觉反馈。
    Jump,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevealRequest {
    pub byte: ByteOffset,
    pub kind: RevealKind,
    /// 单调递增计数。
    /// 同一个字节位置和同一种意图也能被反复触发，渲染端按序号判别是否为新请求。
    pub seq: u64,
}

/// 一个视图：对某个缓冲区（buffer）的一次观察。
#[derive(Debug)]
pub struct View {
    buffer: BufferId,
    selection: SelectionSet,
    folds: FoldSet,
    viewport: ViewportState,
    reveal: Option<RevealRequest>,
    reveal_seq: u64,
    /// 已被 `settle_viewport_y` 消费过的最新显露请求序号。
    /// 避免同一条显露请求被反复应用；新一次 `request_reveal` 会推进序号。
    last_applied_reveal_seq: Option<u64>,
}

/// `View::settle_viewport_y` 的产出：已就位的视口与本帧是否消费了显露请求。
///
/// `viewport` 内容已写入 `self.viewport`，单独返回供调用方读取最终值。
/// `consumed_reveal` 在本帧成功应用 Y 轴显露请求时为 `Some(req)`。
/// 调用方可据此执行 X 轴摆位；为 `None` 时不需要额外动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewportSettlement {
    pub viewport: ViewportState,
    pub consumed_reveal: Option<RevealRequest>,
}

impl View {
    /// 新建视图。
    /// `base_version` 是被观察缓冲区的当前版本。
    /// `FoldSet` 必须版本绑定，因此构造时必须提供。
    pub fn new(buffer: BufferId, base_version: BufferVersion) -> Self {
        Self {
            buffer,
            selection: SelectionSet::default(),
            folds: FoldSet::new(base_version),
            viewport: ViewportState::default(),
            reveal: None,
            reveal_seq: 0,
            last_applied_reveal_seq: None,
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

    /// 请求把 `byte` 滚到视区。`kind` 标明触发场景，具体怎么滚由渲染端解读。
    /// 每次调用都推进序号。
    /// 哪怕同一字节位置、同一种意图也算「新一次请求」，让渲染端有依据再触发一次。
    pub fn request_reveal(&mut self, byte: ByteOffset, kind: RevealKind) {
        self.reveal_seq = self.reveal_seq.wrapping_add(1);
        self.reveal = Some(RevealRequest {
            byte,
            kind,
            seq: self.reveal_seq,
        });
    }

    /// 在 `build_snapshot` 之前调用。
    /// 根据待处理的显露请求与选区行号落定 `viewport.top_line`。
    /// 后续快照会据此切出正确的窗口范围。
    ///
    /// 调用方负责把以下值预解析好传入（视图层不持有缓冲区）：
    /// - `total_lines`：缓冲区的逻辑行总数；
    /// - `selection_head_line`：主光标当前所在逻辑行（0-based）；
    /// - `reveal_line`：若有待处理显露请求，把 `reveal.byte` 折成逻辑行后传入；
    ///   `byte_to_position` 失败时传 `None`。
    ///   本次请求视为过期：跳过 Y 摆位，但仍推进 `last_applied_reveal_seq` 防止反复重试。
    ///
    /// `viewport.visible_line_count` 由 `View::new` 初始化为 `DEFAULT_INITIAL_VISIBLE_LINES`。
    /// 渲染端会同步回写真实值。
    /// 本函数内部对其取 `.max(1)` 仅作除零防御，不再有「未测量」特殊路径。
    ///
    /// 算法：
    /// 1. **显露请求路径**：若 `reveal.seq > last_applied_reveal_seq`，先检查 `RevealKind`。
    ///    `Jump` 强制把 `top_line` 摆到 `reveal_line` 的视区上 1/3。
    ///    `Match` 仅当目标不在 `[top_line, top_line + visible)` 时摆位。
    /// 2. **边缘滚动防御**：检查 `selection_head_line` 是否在 `[top_line, top_line + visible)`。
    ///    若不在，把 `top_line` 调到刚好包含光标。
    /// 3. **范围裁剪**：把 `top_line` 夹到 `[0, total_lines - visible]`。
    pub fn settle_viewport_y(
        &mut self,
        total_lines: u64,
        selection_head_line: u64,
        reveal_line: Option<u64>,
    ) -> ViewportSettlement {
        let visible = self.viewport.visible_line_count.max(1);

        let mut top = self.viewport.top_line;

        // 1. 显露请求路径。
        let mut consumed_reveal = None;
        if let Some(req) = self.reveal
            && Some(req.seq) != self.last_applied_reveal_seq
        {
            if let Some(row) = reveal_line {
                let in_view = row >= top && row < top.saturating_add(visible);
                let force = matches!(req.kind, RevealKind::Jump);
                if force || !in_view {
                    // 上 1/3 摆位，向下整数除法即可。
                    let upper_third = visible / 3;
                    top = row.saturating_sub(upper_third);
                }
            }
            // 即使 reveal_line == None 也推进序号，避免下一帧补一次延迟反应。
            self.last_applied_reveal_seq = Some(req.seq);
            consumed_reveal = Some(req);
        }

        // 2. 边缘滚动防御。
        let cursor = selection_head_line;
        if cursor < top {
            top = cursor;
        } else if cursor >= top.saturating_add(visible) {
            top = cursor.saturating_sub(visible.saturating_sub(1));
        }

        // 3. 裁剪到 [0, total_lines - visible]。
        let max_top = total_lines.saturating_sub(visible);
        if top > max_top {
            top = max_top;
        }

        self.viewport.top_line = top;
        ViewportSettlement {
            viewport: self.viewport,
            consumed_reveal,
        }
    }
}

/// 全部视图的集合，并记录当前活动视图。
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

    /// 为某个缓冲区新建视图；若当前没有活动视图，则设为活动。
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

#[cfg(test)]
mod settle_tests {
    use super::*;
    use zom_engine::ByteOffset;

    fn fresh_view() -> View {
        // 走 ViewSet 构造 BufferId（其内部字段对外私有）。
        let mut views = ViewSet::new();
        let id = views.open_view(views_first_buffer_id(), BufferVersion::INITIAL);
        // 测试需要持有独立的 `View`。
        let view = views.view(id).unwrap();
        View::new(view.buffer(), BufferVersion::INITIAL)
    }

    /// `ViewSet::open_view` 需要一个 `BufferId`；用工作区真实构造一个最简的。
    fn views_first_buffer_id() -> BufferId {
        let mut ws = zom_workspace::Workspace::new();
        ws.open_text(None, "".to_string()).unwrap()
    }

    fn with_viewport(top: u64, visible: u64) -> View {
        let mut v = fresh_view();
        v.set_viewport(ViewportState {
            top_line: top,
            visible_line_count: visible,
        });
        v
    }

    #[test]
    fn settle_should_leave_top_unchanged_when_cursor_in_view_and_no_reveal() {
        let mut view = with_viewport(100, 40);
        let out = view.settle_viewport_y(10_000, 120, None);
        assert_eq!(out.viewport.top_line, 100);
        assert_eq!(out.consumed_reveal, None);
    }

    #[test]
    fn settle_should_edge_scroll_down_when_cursor_below_view() {
        let mut view = with_viewport(100, 40);
        // 光标在第 200 行，视口 [100, 140)，应推到 [161, 201)。
        let out = view.settle_viewport_y(10_000, 200, None);
        assert_eq!(out.viewport.top_line, 200 - 40 + 1);
    }

    #[test]
    fn settle_should_edge_scroll_up_when_cursor_above_view() {
        let mut view = with_viewport(500, 40);
        let out = view.settle_viewport_y(10_000, 200, None);
        assert_eq!(out.viewport.top_line, 200);
    }

    #[test]
    fn settle_should_apply_jump_reveal_to_upper_third_even_when_target_visible() {
        let mut view = with_viewport(100, 30);
        view.request_reveal(ByteOffset::new(0), RevealKind::Jump);
        // 显露目标在第 110 行，本来已在视区 [100, 130)，`Jump` 强制上 1/3。
        let out = view.settle_viewport_y(10_000, 110, Some(110));
        // upper_third = 30 / 3 = 10；top = 110 - 10 = 100。
        assert_eq!(out.viewport.top_line, 100);
        assert!(out.consumed_reveal.is_some());
    }

    #[test]
    fn settle_should_apply_match_reveal_only_when_target_not_visible() {
        let mut view = with_viewport(100, 30);
        view.request_reveal(ByteOffset::new(0), RevealKind::Match);
        // 目标在第 50000 行，远离视区。
        let out = view.settle_viewport_y(100_000, 50_000, Some(50_000));
        // upper_third = 10；top = 49_990。
        assert_eq!(out.viewport.top_line, 49_990);
    }

    #[test]
    fn settle_should_skip_match_reveal_when_target_already_visible() {
        let mut view = with_viewport(100, 30);
        view.request_reveal(ByteOffset::new(0), RevealKind::Match);
        // 目标在第 115 行，已在视区 [100, 130) 内；`Match` 不强制摆位。
        let out = view.settle_viewport_y(10_000, 115, Some(115));
        assert_eq!(out.viewport.top_line, 100);
        // 但序号仍然被消费。
        assert!(out.consumed_reveal.is_some());
    }

    #[test]
    fn settle_should_not_reapply_same_reveal_seq() {
        let mut view = with_viewport(100, 30);
        view.request_reveal(ByteOffset::new(0), RevealKind::Jump);
        let first = view.settle_viewport_y(10_000, 100, Some(5_000));
        assert!(first.consumed_reveal.is_some());
        // 第二次相同帧调用：序号已经消费过。
        let second = view.settle_viewport_y(10_000, 100, Some(5_000));
        assert_eq!(second.consumed_reveal, None);
    }

    #[test]
    fn settle_should_advance_seq_even_when_reveal_line_is_unresolved() {
        let mut view = with_viewport(100, 30);
        view.request_reveal(ByteOffset::new(0), RevealKind::Jump);
        // `reveal_line == None`：`byte_to_position` 失败，跳过 Y 摆位但仍消费序号。
        let out = view.settle_viewport_y(10_000, 100, None);
        assert_eq!(out.viewport.top_line, 100); // 未摆位
        assert!(out.consumed_reveal.is_some());

        // 下一次相同 reveal 不会被重复应用
        let next = view.settle_viewport_y(10_000, 100, Some(5_000));
        assert_eq!(next.consumed_reveal, None);
    }

    #[test]
    fn fresh_view_should_carry_default_initial_visible_lines() {
        // `View::new` 不应再让 `visible_line_count == 0`；下游消费侧不必再做「未测量」特殊路径。
        let view = fresh_view();
        assert_eq!(
            view.viewport().visible_line_count,
            DEFAULT_INITIAL_VISIBLE_LINES
        );
    }

    #[test]
    fn settle_should_clamp_visible_zero_to_one_as_division_guard() {
        // 调用方故意把 `visible_line_count` 设为 0 时，`settle` 不应除以 0；
        // 内部 `.max(1)` 防御会让行为退化为 1 行视口。
        let mut view = with_viewport(0, 0);
        let out = view.settle_viewport_y(1_000, 500, None);
        // visible=1，光标 500 不在 [0, 1)，边缘滚动会推到 top = 500。
        assert_eq!(out.viewport.top_line, 500);
    }

    #[test]
    fn settle_should_clamp_top_to_max_when_cursor_near_end() {
        let mut view = with_viewport(0, 40);
        // 光标在文件末尾第 99 行；max_top = 100 - 40 = 60。
        let out = view.settle_viewport_y(100, 99, None);
        assert_eq!(out.viewport.top_line, 60);
    }

    #[test]
    fn settle_should_handle_reveal_target_near_end_with_clamp() {
        let mut view = with_viewport(0, 30);
        view.request_reveal(ByteOffset::new(0), RevealKind::Jump);
        // 目标在第 95 行（视为光标也在 95），upper_third = 10，期望 top = 85；
        // max_top = 100 - 30 = 70，最终裁剪到 70。
        let out = view.settle_viewport_y(100, 95, Some(95));
        assert_eq!(out.viewport.top_line, 70);
    }
}
