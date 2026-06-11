//! 编辑面状态层。
//!
//! 持有「我怎么看一个缓冲区（buffer）」的状态。
//! 包括看哪个缓冲区、滚到哪、本视图的光标与折叠。
//! 判据：同一文件开两个分屏会不同的状态归这里。
//! 属于文件本身的状态归 `zom-workspace`。
//!
//! `SelectionSet` / `FoldSet` 的实例归视图层持有。
//! `zom-engine` 提供类型、光标移动与编辑后状态转移算法。
//!
//! 视图分两类，统一活在 [`ViewSet`] 的同一 id 空间下：
//! - [`EditView`]：对 buffer 的可编辑视角；持选区、视口、wrap_map、折叠等。
//! - [`PreviewView`]：对 buffer 的只读渲染视角（如 Markdown 预览）；只持 buffer id。

use std::collections::BTreeMap;

use zom_engine::{Buffer, BufferVersion, ByteOffset, FoldSet, SelectionSet};
use zom_workspace::BufferId;

mod wrap;

pub use wrap::{VisualAffinity, VisualPosition, WrapMap, compute_segments};

/// 视图标识。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ViewId(u64);

impl ViewId {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// 视口尚未由渲染端测量过时，`EditView::new` 给可见行数的初始估值。
///
/// 取一个接近常见大屏首帧需求、又不会让小缓冲区过度分配的值。
/// 第一帧渲染元素测出真实值后会同步回写覆盖。
pub const DEFAULT_INITIAL_VISIBLE_LINES: u64 = 200;

/// 滚动位置 / 可见区域。
///
/// `top_line` 是当前视口顶部可见视觉段所属的逻辑行（0-based）。
/// `top_subrow` 是该逻辑行内的软换行视觉段序号（0-based）。
/// 不开软换行时 `top_subrow` 恒为 0。
///
/// `top_line` 的真源是视图自身，由 `settle_viewport_y` 落定。
/// `visible_visual_rows` 与 `visible_logical_lines` 由渲染端按边界、行高和软换行结果测量后同步回写。
/// `EditView::new` 给二者非零初值。
/// 消费侧不需要再为「还没测过」保留特殊路径。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewportState {
    /// 顶部可见视觉段所属的逻辑行（0-based）。
    pub top_line: u64,
    /// 顶部可见视觉段在 `top_line` 内的 sub-row 序号（0-based）。
    pub top_subrow: u64,
    /// 视口能容纳的视觉行数。
    pub visible_visual_rows: u64,
    /// 下一帧 snapshot 至少需要切出的逻辑行数。
    pub visible_logical_lines: u64,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            top_line: 0,
            top_subrow: 0,
            visible_visual_rows: DEFAULT_INITIAL_VISIBLE_LINES,
            visible_logical_lines: DEFAULT_INITIAL_VISIBLE_LINES,
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

/// 视图种类——编辑视图与预览视图共用 ViewId 空间，但语义不同。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ViewKind {
    Edit,
    Preview,
}

/// 一个视图：对某个缓冲区（buffer）的一次观察。
///
/// `View` 是枚举，区分编辑视角（`Edit`）与预览视角（`Preview`）。
/// 上层用统一的 [`ViewId`] 标识，按"一 view = 一 tab"驱动 UI。
/// 编辑命令通过 [`View::as_edit_mut`] / [`ViewSet::edit_view_mut`] 拿到 [`EditView`] 后操作；
/// 预览视图当前只持 buffer id，将来要加滚动位置 / 渲染参数都加到 [`PreviewView`]。
#[derive(Debug)]
pub enum View {
    Edit(EditView),
    Preview(PreviewView),
}

impl View {
    pub fn kind(&self) -> ViewKind {
        match self {
            View::Edit(_) => ViewKind::Edit,
            View::Preview(_) => ViewKind::Preview,
        }
    }

    pub fn buffer(&self) -> BufferId {
        match self {
            View::Edit(v) => v.buffer(),
            View::Preview(v) => v.buffer(),
        }
    }

    pub fn as_edit(&self) -> Option<&EditView> {
        match self {
            View::Edit(v) => Some(v),
            View::Preview(_) => None,
        }
    }

    pub fn as_edit_mut(&mut self) -> Option<&mut EditView> {
        match self {
            View::Edit(v) => Some(v),
            View::Preview(_) => None,
        }
    }

    pub fn as_preview(&self) -> Option<&PreviewView> {
        match self {
            View::Preview(v) => Some(v),
            View::Edit(_) => None,
        }
    }
}

/// 只读渲染视角——只标记"这条 tab 是某 buffer 的预览"，
/// 不持选区 / 视口等编辑态。
#[derive(Debug)]
pub struct PreviewView {
    buffer: BufferId,
}

impl PreviewView {
    pub fn new(buffer: BufferId) -> Self {
        Self { buffer }
    }

    pub fn buffer(&self) -> BufferId {
        self.buffer
    }
}

/// 编辑视图：可写、有选区与视口、参与命令派发。
///
/// 视觉模型采用 zom-engine 的 [`WrapMap`]：渲染端按字体度量算好行内断点，
/// 整篇同步落到 `wrap_map`；命令层从 view 取出后在文本域查询，不依赖帧渲染节奏。
/// `visual_caret` 是 primary caret 的视觉投影，`goal_column` 是连续上下移动的 sticky 列；
/// 二者一起替代旧的 `VisualCaretState`。
#[derive(Debug)]
pub struct EditView {
    buffer: BufferId,
    selection: SelectionSet,
    folds: FoldSet,
    viewport: ViewportState,
    visual_caret: Option<VisualPosition>,
    goal_column: Option<u32>,
    wrap_map: Option<WrapMap>,
    reveal: Option<RevealRequest>,
    reveal_seq: u64,
    /// 已被 `settle_viewport_y` 消费过的最新显露请求序号。
    /// 避免同一条显露请求被反复应用；新一次 `request_reveal` 会推进序号。
    last_applied_reveal_seq: Option<u64>,
    /// 用户手动滚轮滚动后，允许 caret 暂时离开视区。
    /// 当 selection head 变化时，下一次 settle 会恢复 caret edge-scroll。
    manual_scroll_anchor: Option<ByteOffset>,
}

/// `EditView::settle_viewport_y` 的产出：已就位的视口与本帧是否消费了显露请求。
///
/// `viewport` 内容已写入 `self.viewport`，单独返回供调用方读取最终值。
/// `consumed_reveal` 在本帧成功应用 Y 轴显露请求时为 `Some(req)`。
/// 调用方可据此执行 X 轴摆位；为 `None` 时不需要额外动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewportSettlement {
    pub viewport: ViewportState,
    pub consumed_reveal: Option<RevealRequest>,
}

impl EditView {
    /// 新建视图。
    /// `base_version` 是被观察缓冲区的当前版本。
    /// `FoldSet` 必须版本绑定，因此构造时必须提供。
    pub fn new(buffer: BufferId, base_version: BufferVersion) -> Self {
        Self {
            buffer,
            selection: SelectionSet::default(),
            folds: FoldSet::new(base_version),
            viewport: ViewportState::default(),
            visual_caret: None,
            goal_column: None,
            wrap_map: None,
            reveal: None,
            reveal_seq: 0,
            last_applied_reveal_seq: None,
            manual_scroll_anchor: None,
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

    pub fn wrap_map(&self) -> Option<&WrapMap> {
        self.wrap_map.as_ref()
    }

    pub fn set_wrap_map(&mut self, map: Option<WrapMap>) {
        self.wrap_map = map;
    }

    pub fn visual_caret(&self) -> Option<&VisualPosition> {
        self.visual_caret.as_ref()
    }

    pub fn goal_column(&self) -> Option<u32> {
        self.goal_column
    }

    /// 命令完成移动后写回 primary caret 的视觉投影。
    /// `goal_column == None` 表示本次移动不属于"连续上下移动"，下一次垂直移动重新取列。
    pub fn set_visual_caret(&mut self, caret: Option<VisualPosition>, goal_column: Option<u32>) {
        self.visual_caret = caret;
        self.goal_column = goal_column;
    }

    /// 清除 primary caret 的视觉投影（横向移动 / 编辑 / select-all / undo/redo / IME / cut/paste 都走这条）。
    pub fn clear_visual_caret(&mut self) {
        self.visual_caret = None;
        self.goal_column = None;
    }

    /// 把垂直移动需要的字段一次性借出（避免 RefCell-style 多次借用）。
    pub fn vertical_movement_state_mut(
        &mut self,
    ) -> (
        &mut SelectionSet,
        &mut Option<VisualPosition>,
        &mut Option<u32>,
        Option<&WrapMap>,
    ) {
        (
            &mut self.selection,
            &mut self.visual_caret,
            &mut self.goal_column,
            self.wrap_map.as_ref(),
        )
    }

    pub fn reveal(&self) -> Option<RevealRequest> {
        self.reveal
    }

    /// 请求把 `byte` 滚到视区。`kind` 标明触发场景，具体怎么滚由渲染端解读。
    /// 每次调用都推进序号。
    /// 哪怕同一字节位置、同一种意图也算「新一次请求」，让渲染端有依据再触发一次。
    pub fn request_reveal(&mut self, byte: ByteOffset, kind: RevealKind) {
        self.manual_scroll_anchor = None;
        self.reveal_seq = self.reveal_seq.wrapping_add(1);
        self.reveal = Some(RevealRequest {
            byte,
            kind,
            seq: self.reveal_seq,
        });
    }

    /// 用户滚轮滚动：直接移动视口顶端，不改变 selection。
    ///
    /// `delta_visual_rows > 0` 表示向文档后方滚动，`< 0` 表示向文档前方滚动。
    /// 有 wrap map 时在视觉行坐标系里滚；无 wrap map 时退化为逻辑行滚动。
    pub fn scroll_visual_rows(&mut self, buffer: &Buffer, delta_visual_rows: i64) -> bool {
        if delta_visual_rows == 0 {
            return false;
        }

        let visible = self.viewport.visible_visual_rows.max(1);
        let wrap_map = self.wrap_map.as_ref();
        let total_visual_rows = wrap_map
            .map(WrapMap::total_visual_rows)
            .unwrap_or(buffer.line_count() as u64);
        let max_top = total_visual_rows.saturating_sub(visible);

        let top = match wrap_map {
            Some(wm) => wm.visual_row_of(self.viewport.top_line, self.viewport.top_subrow as u32),
            None => self.viewport.top_line,
        };
        let next_top = if delta_visual_rows > 0 {
            top.saturating_add(delta_visual_rows as u64).min(max_top)
        } else {
            top.saturating_sub(delta_visual_rows.unsigned_abs())
        };

        if next_top == top {
            return false;
        }

        let (top_line, top_subrow) = match wrap_map {
            Some(wm) => {
                let (line, subrow) = wm.visual_row_to_line_subrow(next_top);
                (line, subrow as u64)
            }
            None => (next_top, 0),
        };
        self.viewport.top_line = top_line;
        self.viewport.top_subrow = top_subrow;
        self.manual_scroll_anchor = Some(self.selection.primary().head());
        true
    }

    /// 在 `build_snapshot` 之前调用。
    /// 根据待处理的显露请求与光标位置落定 `viewport.top_line` / `viewport.top_subrow`。
    /// 后续快照会据此切出正确的窗口范围。
    ///
    /// 调用方只传入 buffer 与主光标 byte；view 内部按当前 `wrap_map` 折算绝对视觉行。
    /// 有 `wrap_map` 时，软换行 sub-row 也算一条视觉行；无 `wrap_map` 的首帧 / dirty 帧只消费 reveal，跳过 cursor edge-scroll，避免用逻辑行近似误滚。
    ///
    /// 视觉行坐标系是「软换行 sub-row 也算 1 行」的统一空间——光标无论落在逻辑行哪一段都能被精确判定是否越界，不会出现「光标落在逻辑行最后一个 sub-row 上但该 sub-row 已被裁切」的视觉不一致。
    ///
    /// `viewport.visible_visual_rows` 由 `EditView::new` 初始化为 `DEFAULT_INITIAL_VISIBLE_LINES`；
    /// 渲染端会按真实视口高度与行高反算「完整可见的视觉行数」同步回写。
    /// 本函数内部对其取 `.max(1)` 仅作除零防御。
    ///
    /// 算法：
    /// 1. 把 `(top_line, top_subrow)` 经 `wrap_map.visual_row_of` 折成 `top_visual_row`；
    /// 无 `wrap_map` 时即 `top_line`。
    /// 2. **显露请求路径**：若 `reveal.seq > last_applied_reveal_seq`，先按 `RevealKind` 判定。
    /// `Jump` 强制把 `top_visual_row` 摆到 reveal 目标视觉行的视区上 1/3；
    /// `Match` 仅当目标不在 `[top, top + visible)` 时摆位。
    /// 3. **边缘滚动防御**：仅在已有 `wrap_map`（即渲染端完成过测量）时检查
    /// 主光标视觉行是否在 `[top, top + visible)`；不在时调到刚好包含光标。
    /// 无 `wrap_map` 的首帧 / dirty 帧只消费 reveal，跳过 cursor edge-scroll，避免用逻辑行近似误滚。
    /// 4. **范围裁剪**：把 `top_visual_row` 夹到文档可表示范围内。
    /// 5. 把 `top_visual_row` 经 `wrap_map.visual_row_to_line_subrow` 还原回
    /// `(top_line, top_subrow)`；无 `wrap_map` 时 `top_subrow = 0`。
    pub fn settle_viewport_y(
        &mut self,
        buffer: &Buffer,
        selection_head: ByteOffset,
    ) -> ViewportSettlement {
        let visible = self.viewport.visible_visual_rows.max(1);

        if self
            .manual_scroll_anchor
            .is_some_and(|anchor| anchor != selection_head)
        {
            self.manual_scroll_anchor = None;
        }

        let wrap_map = self.wrap_map.as_ref();
        let can_edge_scroll =
            wrap_map.is_some() && self.manual_scroll_anchor != Some(selection_head);
        let total_visual_rows = wrap_map
            .map(WrapMap::total_visual_rows)
            .unwrap_or(buffer.line_count() as u64);
        let cursor_visual_row = wrap_map.and_then(|wm| {
            wm.resolve(buffer, selection_head, None)
                .ok()
                .map(|pos| wm.visual_row_of(pos.logical_line, pos.subrow))
        });

        // 顶部位置折叠到「绝对视觉行」坐标系。无 wrap_map 时 top_visual_row == top_line。
        let mut top = match wrap_map {
            Some(wm) => wm.visual_row_of(self.viewport.top_line, self.viewport.top_subrow as u32),
            None => self.viewport.top_line,
        };

        // 1. 显露请求路径。
        let mut consumed_reveal = None;
        if let Some(req) = self.reveal
            && Some(req.seq) != self.last_applied_reveal_seq
        {
            let reveal_visual_row = match wrap_map {
                Some(wm) => wm
                    .resolve(buffer, req.byte, None)
                    .ok()
                    .map(|pos| wm.visual_row_of(pos.logical_line, pos.subrow)),
                None => buffer
                    .byte_to_line(req.byte)
                    .ok()
                    .map(|line| line.get() as u64),
            };
            if let Some(row) = reveal_visual_row {
                let in_view = row >= top && row < top.saturating_add(visible);
                let force = matches!(req.kind, RevealKind::Jump);
                if force || !in_view {
                    // 上 1/3 摆位，向下整数除法即可。
                    let upper_third = visible / 3;
                    top = row.saturating_sub(upper_third);
                }
            }
            // 即使 reveal_visual_row == None 也推进序号，避免下一帧补一次延迟反应。
            self.last_applied_reveal_seq = Some(req.seq);
            consumed_reveal = Some(req);
        }

        // 2. 边缘滚动防御。
        if can_edge_scroll && let Some(cursor) = cursor_visual_row {
            if cursor < top {
                top = cursor;
            } else if cursor >= top.saturating_add(visible) {
                top = cursor.saturating_sub(visible.saturating_sub(1));
            }
        }

        // 3. 裁剪到 [0, total_visual_rows - visible]。
        let max_top = total_visual_rows.saturating_sub(visible);
        if top > max_top {
            top = max_top;
        }

        // 4. 写回 (top_line, top_subrow)。无 wrap_map 时 top_subrow 恒为 0。
        let (new_top_line, new_top_subrow) = match wrap_map {
            Some(wm) => {
                let (line, sub) = wm.visual_row_to_line_subrow(top);
                (line, sub as u64)
            }
            None => (top, 0),
        };
        self.viewport.top_line = new_top_line;
        self.viewport.top_subrow = new_top_subrow;
        ViewportSettlement {
            viewport: self.viewport,
            consumed_reveal,
        }
    }
}

/// 全部视图的集合。
///
/// 编辑视图与预览视图共用 id 空间——按打开顺序遍历即"tab 顺序"。
/// `ViewSet` 是纯集合，不记录"哪个 view 是活动的"。
/// 活动状态由上层（`WorkspaceSession::active_view` / `CommandContext::active_view_id`）按需提供。
#[derive(Debug, Default)]
pub struct ViewSet {
    next_view_id: u64,
    views: BTreeMap<ViewId, View>,
}

impl ViewSet {
    pub fn new() -> Self {
        Self {
            next_view_id: 1,
            views: BTreeMap::new(),
        }
    }

    /// 为某个缓冲区新建可编辑视图。
    pub fn open_edit_view(&mut self, buffer: BufferId, base_version: BufferVersion) -> ViewId {
        let id = self.next_id();
        self.views
            .insert(id, View::Edit(EditView::new(buffer, base_version)));
        id
    }

    /// 为某个缓冲区新建预览视图。
    pub fn open_preview_view(&mut self, buffer: BufferId) -> ViewId {
        let id = self.next_id();
        self.views
            .insert(id, View::Preview(PreviewView::new(buffer)));
        id
    }

    fn next_id(&mut self) -> ViewId {
        let id = ViewId(self.next_view_id);
        self.next_view_id += 1;
        id
    }

    /// 关闭视图。上层若把它当作活动 view，需要自己在 close 后挑选下一个候选。
    pub fn close_view(&mut self, id: ViewId) {
        self.views.remove(&id);
    }

    pub fn view(&self, id: ViewId) -> Option<&View> {
        self.views.get(&id)
    }

    pub fn view_mut(&mut self, id: ViewId) -> Option<&mut View> {
        self.views.get_mut(&id)
    }

    /// 拿编辑视图——非编辑 view 返回 None。
    pub fn edit_view(&self, id: ViewId) -> Option<&EditView> {
        self.views.get(&id).and_then(View::as_edit)
    }

    /// 拿编辑视图的可变借用——非编辑 view 返回 None。
    pub fn edit_view_mut(&mut self, id: ViewId) -> Option<&mut EditView> {
        self.views.get_mut(&id).and_then(View::as_edit_mut)
    }

    pub fn views(&self) -> impl Iterator<Item = (ViewId, &View)> {
        self.views.iter().map(|(id, view)| (*id, view))
    }

    /// 反查：第一个跟踪指定 buffer 的编辑视图（按 ViewId 升序）。
    ///
    /// 当前 zom-desktop 是 1 buffer:1 edit view，所以"第一个"即"唯一"。
    /// 未来若同一 buffer 开多个编辑 view，调用方自行扩展。
    pub fn find_edit_view_for_buffer(&self, buffer: BufferId) -> Option<ViewId> {
        self.views.iter().find_map(|(id, view)| {
            matches!(view, View::Edit(v) if v.buffer() == buffer).then_some(*id)
        })
    }

    /// 反查：跟踪指定 buffer 的预览视图（同一 buffer 至多一条预览）。
    pub fn find_preview_view_for_buffer(&self, buffer: BufferId) -> Option<ViewId> {
        self.views.iter().find_map(|(id, view)| {
            matches!(view, View::Preview(v) if v.buffer() == buffer).then_some(*id)
        })
    }

    /// 第一个可用的 ViewId（按 ViewId 升序）。
    /// 给"关掉活动 view 后挑下一个"用。
    pub fn first_view_id(&self) -> Option<ViewId> {
        self.views.keys().next().copied()
    }
}

#[cfg(test)]
mod settle_tests {
    use super::*;
    use zom_engine::{BufferConfig, ByteOffset, Line};

    fn fresh_view() -> EditView {
        // 走 ViewSet 构造 BufferId（其内部字段对外私有）。
        EditView::new(views_first_buffer_id(), BufferVersion::INITIAL)
    }

    /// `ViewSet::open_edit_view` 需要一个 `BufferId`；用工作区真实构造一个最简的。
    fn views_first_buffer_id() -> BufferId {
        let mut ws = zom_workspace::Workspace::new();
        ws.open_text(None, "".to_string()).unwrap()
    }

    fn with_viewport(top: u64, visible: u64) -> EditView {
        let mut v = fresh_view();
        v.set_viewport(ViewportState {
            top_line: top,
            top_subrow: 0,
            visible_visual_rows: visible,
            visible_logical_lines: visible,
        });
        v
    }

    fn with_measured_viewport(top: u64, visible: u64, total_rows: u64) -> EditView {
        let mut v = with_viewport(top, visible);
        v.set_wrap_map(Some(WrapMap::new(
            false,
            vec![Vec::new(); total_rows as usize],
        )));
        v
    }

    fn buffer_with_line_count(line_count: u64) -> Buffer {
        assert!(line_count > 0);
        let mut text = String::new();
        for line in 0..line_count {
            if line > 0 {
                text.push('\n');
            }
            text.push('x');
        }
        Buffer::from_text(text, BufferConfig::default()).unwrap()
    }

    fn buffer_from_lines(lines: &[&str]) -> Buffer {
        Buffer::from_text(lines.join("\n"), BufferConfig::default()).unwrap()
    }

    fn line_start(buffer: &Buffer, line: u64) -> ByteOffset {
        buffer.line_start_byte(Line::new(line as usize)).unwrap()
    }

    #[test]
    fn settle_should_leave_top_unchanged_when_cursor_in_view_and_no_reveal() {
        let mut view = with_viewport(100, 40);
        let buffer = buffer_with_line_count(10_000);
        let out = view.settle_viewport_y(&buffer, line_start(&buffer, 120));
        assert_eq!(out.viewport.top_line, 100);
        assert_eq!(out.consumed_reveal, None);
    }

    #[test]
    fn settle_should_edge_scroll_down_when_cursor_below_view() {
        let mut view = with_measured_viewport(100, 40, 10_000);
        let buffer = buffer_with_line_count(10_000);
        // 光标在第 200 行，视口 [100, 140)，应推到 [161, 201)。
        let out = view.settle_viewport_y(&buffer, line_start(&buffer, 200));
        assert_eq!(out.viewport.top_line, 200 - 40 + 1);
    }

    #[test]
    fn settle_should_edge_scroll_up_when_cursor_above_view() {
        let mut view = with_measured_viewport(500, 40, 10_000);
        let buffer = buffer_with_line_count(10_000);
        let out = view.settle_viewport_y(&buffer, line_start(&buffer, 200));
        assert_eq!(out.viewport.top_line, 200);
    }

    #[test]
    fn settle_should_skip_edge_scroll_until_wrap_map_is_measured() {
        let mut view = with_viewport(100, 40);
        let buffer = buffer_with_line_count(10_000);
        let out = view.settle_viewport_y(&buffer, line_start(&buffer, 200));
        assert_eq!(out.viewport.top_line, 100);
        assert_eq!(out.consumed_reveal, None);
    }

    #[test]
    fn settle_should_apply_jump_reveal_to_upper_third_even_when_target_visible() {
        let mut view = with_viewport(100, 30);
        let buffer = buffer_with_line_count(10_000);
        view.request_reveal(line_start(&buffer, 110), RevealKind::Jump);
        // 显露目标在第 110 行，本来已在视区 [100, 130)，`Jump` 强制上 1/3。
        let out = view.settle_viewport_y(&buffer, line_start(&buffer, 110));
        // upper_third = 30 / 3 = 10；top = 110 - 10 = 100。
        assert_eq!(out.viewport.top_line, 100);
        assert!(out.consumed_reveal.is_some());
    }

    #[test]
    fn settle_should_apply_match_reveal_only_when_target_not_visible() {
        let mut view = with_viewport(100, 30);
        let buffer = buffer_with_line_count(100_000);
        view.request_reveal(line_start(&buffer, 50_000), RevealKind::Match);
        // 目标在第 50000 行，远离视区。
        let out = view.settle_viewport_y(&buffer, line_start(&buffer, 50_000));
        // upper_third = 10；top = 49_990。
        assert_eq!(out.viewport.top_line, 49_990);
    }

    #[test]
    fn settle_should_skip_match_reveal_when_target_already_visible() {
        let mut view = with_viewport(100, 30);
        let buffer = buffer_with_line_count(10_000);
        view.request_reveal(line_start(&buffer, 115), RevealKind::Match);
        // 目标在第 115 行，已在视区 [100, 130) 内；`Match` 不强制摆位。
        let out = view.settle_viewport_y(&buffer, line_start(&buffer, 115));
        assert_eq!(out.viewport.top_line, 100);
        // 但序号仍然被消费。
        assert!(out.consumed_reveal.is_some());
    }

    #[test]
    fn settle_should_not_reapply_same_reveal_seq() {
        let mut view = with_viewport(100, 30);
        let buffer = buffer_with_line_count(10_000);
        view.request_reveal(line_start(&buffer, 5_000), RevealKind::Jump);
        let first = view.settle_viewport_y(&buffer, line_start(&buffer, 100));
        assert!(first.consumed_reveal.is_some());
        // 第二次相同帧调用：序号已经消费过。
        let second = view.settle_viewport_y(&buffer, line_start(&buffer, 100));
        assert_eq!(second.consumed_reveal, None);
    }

    #[test]
    fn settle_should_advance_seq_even_when_reveal_line_is_unresolved() {
        let mut view = with_viewport(100, 30);
        let buffer = buffer_with_line_count(10_000);
        view.request_reveal(ByteOffset::new(usize::MAX), RevealKind::Jump);
        // reveal byte 解析失败：跳过 Y 摆位但仍消费序号。
        let out = view.settle_viewport_y(&buffer, line_start(&buffer, 100));
        assert_eq!(out.viewport.top_line, 100); // 未摆位
        assert!(out.consumed_reveal.is_some());

        // 下一次相同 reveal 不会被重复应用
        let next = view.settle_viewport_y(&buffer, line_start(&buffer, 100));
        assert_eq!(next.consumed_reveal, None);
    }

    #[test]
    fn fresh_view_should_carry_default_initial_visible_lines() {
        // `EditView::new` 不应再让可见行数为 0；下游消费侧不必再做「未测量」特殊路径。
        let view = fresh_view();
        assert_eq!(
            view.viewport().visible_visual_rows,
            DEFAULT_INITIAL_VISIBLE_LINES
        );
        assert_eq!(
            view.viewport().visible_logical_lines,
            DEFAULT_INITIAL_VISIBLE_LINES
        );
    }

    #[test]
    fn settle_should_clamp_visible_zero_to_one_as_division_guard() {
        // 调用方故意把 `visible_logical_lines` 设为 0 时，`settle` 不应除以 0；
        // 内部 `.max(1)` 防御会让行为退化为 1 行视口。
        let mut view = with_measured_viewport(0, 0, 1_000);
        let buffer = buffer_with_line_count(1_000);
        let out = view.settle_viewport_y(&buffer, line_start(&buffer, 500));
        // visible=1，光标 500 不在 [0, 1)，边缘滚动会推到 top = 500。
        assert_eq!(out.viewport.top_line, 500);
    }

    #[test]
    fn settle_should_clamp_top_to_max_when_cursor_near_end() {
        let mut view = with_measured_viewport(0, 40, 100);
        let buffer = buffer_with_line_count(100);
        // 光标在文件末尾第 99 行；max_top = 100 - 40 = 60。
        let out = view.settle_viewport_y(&buffer, line_start(&buffer, 99));
        assert_eq!(out.viewport.top_line, 60);
    }

    #[test]
    fn settle_should_handle_reveal_target_near_end_with_clamp() {
        let mut view = with_viewport(0, 30);
        let buffer = buffer_with_line_count(100);
        view.request_reveal(line_start(&buffer, 95), RevealKind::Jump);
        // 目标在第 95 行（视为光标也在 95），upper_third = 10，期望 top = 85；
        // max_top = 100 - 30 = 70，最终裁剪到 70。
        let out = view.settle_viewport_y(&buffer, line_start(&buffer, 95));
        assert_eq!(out.viewport.top_line, 70);
    }

    #[test]
    fn settle_with_wrap_map_edge_scrolls_in_visual_row_space() {
        // 4 条逻辑行；每条都被切成 2 个 sub-row（共 8 个视觉行）。
        // 视口高度 3 视觉行，初始 top 在 (line=0, subrow=0)。
        let mut view = fresh_view();
        view.set_wrap_map(Some(WrapMap::new(
            true,
            vec![vec![5], vec![5], vec![5], vec![5]],
        )));
        view.set_viewport(ViewportState {
            top_line: 0,
            top_subrow: 0,
            visible_visual_rows: 3,
            visible_logical_lines: 2,
        });
        // 光标在 (line=1, subrow=1) = 视觉行 3，刚好越出 [0, 3)；
        // 期望 top 推到视觉行 1 = (line=0, subrow=1)。
        // 旧的逻辑行算法会把 cursor 当作 line=1，仍在 [0,2) 内不滚——这是修复目标。
        let buffer = buffer_from_lines(&["abcdefghij", "abcdefghij", "abcdefghij", "abcdefghij"]);
        let cursor = ByteOffset::new(line_start(&buffer, 1).get() + 5);
        let cursor_visual = view
            .wrap_map()
            .unwrap()
            .resolve(&buffer, cursor, None)
            .map(|pos| {
                view.wrap_map()
                    .unwrap()
                    .visual_row_of(pos.logical_line, pos.subrow)
            })
            .unwrap();
        assert_eq!(cursor_visual, 3);
        let out = view.settle_viewport_y(&buffer, cursor);
        assert_eq!(out.viewport.top_line, 0);
        assert_eq!(out.viewport.top_subrow, 1);
    }

    #[test]
    fn settle_with_wrap_map_keeps_top_when_cursor_in_view() {
        // 光标已经在视口内，top 应保持不动（包括 top_subrow）。
        let mut view = fresh_view();
        view.set_wrap_map(Some(WrapMap::new(true, vec![vec![5], vec![5], vec![5]])));
        view.set_viewport(ViewportState {
            top_line: 1,
            top_subrow: 0,
            visible_visual_rows: 2,
            visible_logical_lines: 1,
        });
        // top_visual_row = 2，视口 [2, 4)，光标在 (line=1, subrow=1) = 视觉行 3。
        let buffer = buffer_from_lines(&["abcdefghij", "abcdefghij", "abcdefghij"]);
        let cursor = ByteOffset::new(line_start(&buffer, 1).get() + 5);
        let out = view.settle_viewport_y(&buffer, cursor);
        assert_eq!(out.viewport.top_line, 1);
        assert_eq!(out.viewport.top_subrow, 0);
    }

    #[test]
    fn manual_scroll_should_move_viewport_without_moving_selection() {
        let mut view = with_measured_viewport(10, 5, 100);
        let buffer = buffer_with_line_count(100);

        assert!(view.scroll_visual_rows(&buffer, 3));

        assert_eq!(view.viewport().top_line, 13);
        assert_eq!(view.selection().primary().head(), ByteOffset::ZERO);
    }

    #[test]
    fn settle_should_not_edge_scroll_back_after_manual_scroll_until_cursor_moves() {
        let mut view = with_measured_viewport(0, 5, 100);
        let buffer = buffer_with_line_count(100);
        *view.selection_mut() = SelectionSet::caret(line_start(&buffer, 0));

        assert!(view.scroll_visual_rows(&buffer, 20));
        let after_scroll = view.settle_viewport_y(&buffer, line_start(&buffer, 0));
        assert_eq!(after_scroll.viewport.top_line, 20);

        let after_cursor_move = view.settle_viewport_y(&buffer, line_start(&buffer, 1));
        assert_eq!(after_cursor_move.viewport.top_line, 1);
    }

    #[test]
    fn manual_scroll_with_wrap_map_should_scroll_in_visual_rows() {
        let mut view = fresh_view();
        view.set_wrap_map(Some(WrapMap::new(true, vec![vec![5], vec![5], vec![5]])));
        view.set_viewport(ViewportState {
            top_line: 0,
            top_subrow: 0,
            visible_visual_rows: 2,
            visible_logical_lines: 1,
        });
        let buffer = buffer_from_lines(&["abcdefghij", "abcdefghij", "abcdefghij"]);

        assert!(view.scroll_visual_rows(&buffer, 3));

        assert_eq!(view.viewport().top_line, 1);
        assert_eq!(view.viewport().top_subrow, 1);
    }
}
