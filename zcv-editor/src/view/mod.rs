//! Editor View 的跨帧状态与交互。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::cell::Cell;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, Entity, EventEmitter, FocusHandle, IntoElement,
    KeyContext, Pixels, Point, Render, Styled, TextRun, Window, div, point, prelude::*,
};
use zcv_actions::{
    Backspace, Copy, Cut, Delete, DeleteToBeginningOfLine, DeleteToEndOfLine, DeleteToNextWordEnd,
    DeleteToPreviousWordStart, Indent, MoveDown, MoveLeft, MoveLineDown, MoveLineUp, MovePageDown,
    MovePageUp, MoveRight, MoveToBeginning, MoveToBeginningOfLine, MoveToEnd, MoveToEndOfLine,
    MoveToNextWord, MoveToPreviousWord, MoveUp, Newline, OpenExcerpts, Outdent, Paste, Redo,
    SelectAll, SelectDown, SelectLargerSyntaxNode, SelectLeft, SelectPageDown, SelectPageUp,
    SelectRight, SelectSmallerSyntaxNode, SelectToBeginning, SelectToBeginningOfLine, SelectToEnd,
    SelectToEndOfLine, SelectToNextWord, SelectToPreviousWord, SelectUp, ToggleFold, Undo,
    UnfoldAll,
};
use zcv_language::{AutoClosePair, BracketPair, LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{
    DiffFile, DiffHunkSource, DisplayHunk, ExcerptDiffKind, ExcerptLocation, ExcerptSnapshot,
    MultiBuffer, MultiBufferAnchor, MultiBufferSnapshot, WordDiffs,
};
use zcv_settings::{SettingsStore, SoftWrapMode};
use zcv_text::{
    Affinity, Buffer, BufferConfig, BufferVersion, Line, LineRange, LogicalColumn,
    MovementDirection, MovementUnit, Position, PositionMap, TextError, TextResult, TransactionId,
    TransactionMergePolicy, TransactionMetadata, TransactionSource,
};
use zcv_theme::{color, typography};
use zcv_workspace::typography_for_window;

use crate::scrollbar::{ScrollbarMarker, ScrollbarMarkerState};

use super::blink_manager::BlinkManager;
use super::display_map::{
    CreaseId, DisplayColumn, DisplayMap, DisplayPoint, DisplayRow, DisplaySnapshot, EditorHunk,
    FoldBias, HunkControlTarget, WrapRowKind,
};
use super::element::{AUTOSCROLL_INTERVAL, EditorElement, EditorInputLayout};
use super::scroll::{ScrollManager, ScrollViewport, ScrollbarThumbState};
use super::selection::{
    EditOutcome, EditPlan, Selection, SelectionHistory, SelectionSet, replace_selections,
};

mod presentation;
mod rename;
mod search;
mod syntax;

use rename::LocalRenameState;

pub(crate) use search::EditorSearch;

/// 导航跳转（打开文件/行列定位）时目标行距视口顶部的固定行数，留出上下文。
pub(super) const NAVIGATION_TOP_OFFSET: usize = 4;

/// Editor 自身的领域事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    /// 编辑器关联的文件路径发生变化。
    PathChanged,
    /// 文档内容被编辑；只在真实事务提交时发布。
    Edited { transaction_id: TransactionId },
    /// 文档是否包含未保存修改发生变化。
    DirtyChanged,
    /// 复合文档请求宿主打开底层文件。
    OpenExcerptsRequested {
        locations: Vec<ExcerptLocation>,
        split: bool,
    },
    /// 删除/修改块的展开折叠状态变化（宿主按展开状态重建组合文档内容）。
    DiffHunksExpandedChanged,
    /// 用户主动操作失败，由宿主工作区负责展示。
    Error(String),
}

/// Editor 负责把控件定位到 hunk 右上角，具体按钮与操作由宿主视图提供。
pub trait DiffHunkDelegate {
    fn render_hunk_controls(
        &self,
        _target: &HunkControlTarget,
        _row: usize,
        _editor: &Entity<Editor>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<AnyElement> {
        None
    }

    fn render_buffer_header_controls(
        &self,
        _path: &std::path::Path,
        _sticky: bool,
        _row: usize,
        _editor: &Entity<Editor>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<AnyElement> {
        None
    }
}

/// 一帧渲染使用的只读编辑器快照。
///
/// 聚合显示快照、占位显示快照、滚动锚点与显示选项；
/// 渲染层从同一份快照读取，不在布局过程中分别读取 `Editor` 字段。
/// 可丢弃、可重建，不是新的文档模型；由 [`Editor::snapshot`] 按当前帧派生。
#[derive(Clone)]
pub(crate) struct EditorSnapshot {
    display_snapshot: DisplaySnapshot,
    placeholder_display_snapshot: Option<DisplaySnapshot>,
    mode: EditorMode,
    shows_gutter: bool,
    soft_wrap: SoftWrap,
    preferred_line_length: usize,
    scroll_anchor: DisplayPoint,
    scroll_offset: Point<Pixels>,
    is_focused: bool,
}

impl EditorSnapshot {
    pub(crate) fn display_snapshot(&self) -> &DisplaySnapshot {
        &self.display_snapshot
    }

    pub(crate) fn placeholder_display_snapshot(&self) -> Option<&DisplaySnapshot> {
        self.placeholder_display_snapshot.as_ref()
    }

    pub(crate) fn mode(&self) -> &EditorMode {
        &self.mode
    }

    pub(crate) fn shows_gutter(&self) -> bool {
        self.shows_gutter
    }

    pub(crate) fn soft_wrap(&self) -> SoftWrap {
        self.soft_wrap
    }

    pub(crate) fn preferred_line_length(&self) -> usize {
        self.preferred_line_length
    }

    pub(crate) fn scroll_anchor(&self) -> DisplayPoint {
        self.scroll_anchor
    }

    pub(crate) fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll_offset
    }

    /// 当前帧编辑器是否聚焦（含窗口激活）。
    pub(crate) fn is_focused(&self) -> bool {
        self.is_focused
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    ByUnit(MovementUnit),
    LineStep,
    PageStep(usize),
    DocumentEdge,
}

impl From<MovementUnit> for Motion {
    fn from(unit: MovementUnit) -> Self {
        Self::ByUnit(unit)
    }
}

/// 鼠标手势的选区粒度。
///
/// Word/Line 携带手势起点时的锚定范围：拖拽扩展与 Shift+点击按粒度扩展时，选区以该范围的两端为基准做整词/整行吸附。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MouseSelectMode {
    Character,
    Word(Range<MultiBufferAnchor>),
    Line(Range<MultiBufferAnchor>),
    All,
}

impl MouseSelectMode {
    /// 外部 reload / 基线替换后把鼠标手势的锚定范围显式重锚到当前快照。
    fn reattach(self, snapshot: &MultiBufferSnapshot) -> Self {
        match self {
            Self::Word(range) => Self::Word(reattach_anchor_range(range, snapshot)),
            Self::Line(range) => Self::Line(reattach_anchor_range(range, snapshot)),
            other => other,
        }
    }
}

fn reattach_anchor_range(
    range: Range<MultiBufferAnchor>,
    snapshot: &MultiBufferSnapshot,
) -> Range<MultiBufferAnchor> {
    let start = snapshot
        .reattach_anchor(&range.start)
        .unwrap_or(range.start);
    let end = snapshot.reattach_anchor(&range.end).unwrap_or(range.end);
    start..end
}

/// 拖拽中的选区状态：固定锚点 + 点击时的粒度。
#[derive(Debug, Clone)]
struct PendingSelection {
    /// 按下点源锚点，字符粒度拖拽的固定端。
    anchor: MultiBufferAnchor,
    /// 点击时的粒度与锚定范围。
    mode: MouseSelectMode,
}

struct LineWidthCache {
    version: BufferVersion,
    row: DisplayRow,
    font_id: gpui::FontId,
    font_size: Pixels,
    width: Pixels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditorMode {
    SingleLine,
    AutoHeight {
        min_lines: usize,
        max_lines: Option<usize>,
    },
    Full,
}

/// 软换行模式；仅编辑器内部与测试使用，外部宿主通过设置决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SoftWrap {
    /// 不换行（超长行横向滚动）。
    #[default]
    None,
    /// 超过编辑器文本区宽度换行。
    EditorWidth,
    /// 在 `preferred_line_length` 与编辑器宽度（取小者）处换行。
    Bounded,
}

impl From<SoftWrapMode> for SoftWrap {
    fn from(mode: SoftWrapMode) -> Self {
        match mode {
            SoftWrapMode::None => SoftWrap::None,
            SoftWrapMode::EditorWidth => SoftWrap::EditorWidth,
            SoftWrapMode::Bounded => SoftWrap::Bounded,
        }
    }
}

/// 宿主注入的显式折叠候选身份。
///
/// 身份只用于移除同一候选；范围本身由显示层以组合锚点保存并随文本演进。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExplicitCreaseId(CreaseId);

pub struct Editor {
    multi_buffer: Entity<MultiBuffer>,
    last_dirty: bool,
    display_map: Entity<DisplayMap>,
    mode: EditorMode,
    /// 单行嵌入编辑器是否跟随代码编辑器的内容排版。
    content_typography: bool,
    /// 空 buffer 时显示的提示文本（如提交信息编辑器的"输入提交信息…"）。
    /// 独立 DisplayMap 承载（placeholder 走真实渲染管线，折行/行高一致）。
    placeholder_display_map: Option<Entity<DisplayMap>>,
    selections: SelectionSet<MultiBufferAnchor>,
    selection_history: SelectionHistory,
    /// 结构化选择扩展链；普通选区变更或文本编辑后失效。
    structured_selection_history: Vec<SelectionSet<MultiBufferAnchor>>,
    scroll_manager: ScrollManager,
    composition: Option<EditorComposition>,
    input_layout: Option<EditorInputLayout>,
    pixel_position_of_newest_cursor: Option<Point<Pixels>>,
    last_bounds: Option<Bounds<Pixels>>,
    last_line_height: Option<Pixels>,
    focus: FocusHandle,
    blink_manager: Entity<BlinkManager>,
    blink_manager_initialized: bool,
    /// 全局设置驱动的换行模式（SettingsStore 变化时自动跟随）。
    soft_wrap: SoftWrap,
    /// 换行模式覆盖（`None` 恢复设置值）。
    soft_wrap_override: Option<SoftWrap>,
    preferred_line_length: usize,
    diff_hunk_delegate: Option<Arc<dyn DiffHunkDelegate>>,
    hovered_diff_hunk: Option<usize>,
    /// 拖拽选择自动滚动的限频时间戳（跨帧持久；事件频率可远超帧率，滚动频率需封顶）。
    pub(crate) last_drag_autoscroll: Cell<Instant>,
    /// 文件内搜索状态（搜索条执行过一次搜索后存在，编辑后自动重搜）。
    /// 命中范围作为显示装饰输入注入显示链，显示坐标投影归 DisplayMap 所有。
    search: Option<EditorSearch>,
    /// 匹配括号缓存：键 = (primary head, buffer 版本, 源元数据版本)。
    /// 光标移动或任一版本推进即重查；
    /// 滚动/纯重绘帧直接命中，不再跑 tree-sitter 查询。
    bracket_pair_cache: Option<(MultiBufferOffset, BufferVersion, u64, Option<BracketPair>)>,
    /// 最近一次鼠标手势的选区粒度；Shift+点击时按此粒度扩展。
    mouse_select_mode: MouseSelectMode,
    /// 正在进行的鼠标选区手势；普通选区变更会终止它。
    pending_selection: Option<PendingSelection>,
    /// 当前文件内局部绑定的行内重命名输入会话。
    local_rename: Option<LocalRenameState>,
    /// 自动补全的闭合符标记（输入闭合符时跳过、退格删除整对的数据源）。
    /// 随每次编辑经 PositionMap 推进；区域版本与当前快照失配（未跟踪的外部编辑）时整体失效。
    autoclose_regions: Vec<AutocloseRegion>,
    /// 未换行模式下最长行的像素宽度；按文本版本、显示行和字体失效。
    line_width_cache: Option<LineWidthCache>,
    /// 滚动条慢标记缓存；由显示版本失效、后台计算（对齐 Zed ScrollbarMarkerState）。
    scrollbar_marker_state: ScrollbarMarkerState,
}

impl Editor {
    pub fn single_line(cx: &mut Context<Self>) -> Self {
        let buffer = Buffer::from_text(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");
        // 单行输入编辑器不携带文件路径，语言状态不会启用；独立注册表避免共享可变单例。
        let language_buffer =
            cx.new(|cx| LanguageBuffer::new(buffer, None, Arc::new(LanguageRegistry::new()), cx));
        Self::from_language_buffer(language_buffer, EditorMode::SingleLine, cx)
    }

    /// 创建使用内容排版的单行编辑器；用于嵌入代码编辑器的单行编辑场景。
    pub(crate) fn single_line_with_content_typography(cx: &mut Context<Self>) -> Self {
        let mut editor = Self::single_line(cx);
        editor.content_typography = true;
        editor
    }

    pub fn auto_height(min_lines: usize, max_lines: Option<usize>, cx: &mut Context<Self>) -> Self {
        let buffer = Buffer::from_text(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");
        // 单行输入编辑器不携带文件路径，语言状态不会启用；独立注册表避免共享可变单例。
        let language_buffer =
            cx.new(|cx| LanguageBuffer::new(buffer, None, Arc::new(LanguageRegistry::new()), cx));
        Self::from_language_buffer(
            language_buffer,
            EditorMode::AutoHeight {
                min_lines,
                max_lines,
            },
            cx,
        )
    }

    pub fn for_multi_buffer(multi_buffer: Entity<MultiBuffer>, cx: &mut Context<Self>) -> Self {
        Self::new(multi_buffer, EditorMode::Full, cx)
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 聚焦状态由渲染快照提供时，按同一语义决定 caret 是否可见。
    ///
    /// 只读 Editor 仍保留稳定可见的 caret，使 MultiBuffer 与普通 Editor 共用同一套定位和选区反馈；
    /// 可编辑 Editor 才由 BlinkManager 控制闪烁。
    pub(crate) fn cursor_visible_with_focus(&self, is_focused: bool, cx: &App) -> bool {
        is_focused && (self.is_read_only(cx) || self.blink_manager.read(cx).visible())
    }

    /// 窗口激活与编辑器焦点是两个独立条件，统一在这里决定闪烁生命周期。
    fn sync_cursor_blinking(&mut self, window: &Window, cx: &mut Context<Self>) {
        let should_blink =
            !self.is_read_only(cx) && window.is_window_active() && self.focus.is_focused(window);
        self.blink_manager.update(cx, |manager, cx| {
            if should_blink {
                manager.enable(cx);
            } else {
                manager.disable(cx);
            }
        });
    }

    pub fn multi_buffer(&self) -> Entity<MultiBuffer> {
        self.multi_buffer.clone()
    }

    pub fn is_read_only(&self, cx: &App) -> bool {
        self.multi_buffer.read(cx).is_read_only()
    }

    pub fn excerpt_location(&self, cx: &App) -> Option<ExcerptLocation> {
        let range = self.resolved_selections(cx).primary().range();
        self.multi_buffer.read(cx).location_for_range(range)
    }

    pub(crate) fn open_excerpt(
        &mut self,
        excerpt: &ExcerptSnapshot,
        split: bool,
        cx: &mut Context<Self>,
    ) {
        let start = excerpt.source_range().start();
        cx.emit(EditorEvent::OpenExcerptsRequested {
            locations: vec![ExcerptLocation {
                path: excerpt.path().to_path_buf(),
                // Header 的 Open File 是“跳到 excerpt 起点”，不是选中整段 excerpt。
                source_range: MultiBufferRange::new(start, start)
                    .expect("同点源范围必须有效")
                    .into(),
            }],
            split,
        });
    }

    pub fn is_buffer_folded(&self, path: &std::path::Path, cx: &App) -> bool {
        self.display_map.read(cx).is_buffer_folded(path)
    }

    /// 折叠/展开 MultiBuffer 中一个文件的全部 excerpts。
    /// 这是 BlockMap 变换，不修改组合文本，也不借用语法折叠范围。
    pub fn toggle_buffer_fold(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let folded = !self.display_map.read(cx).is_buffer_folded(&path);
        self.display_map
            .update(cx, |map, cx| map.set_buffer_folded(path, folded, cx));
        // 滚动位置是长期组合锚点，显示拓扑重建后按当前快照解析即自动落回原内容位置。
        self.advance_snapshots(cx);
        self.input_layout = None;
        cx.notify();
    }

    pub(super) fn handle_open_excerpts(
        &mut self,
        _: &OpenExcerpts,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.multi_buffer.read(cx).singleton_source().is_some() {
            cx.propagate();
            return;
        }
        let locations = self
            .resolved_selections(cx)
            .as_slice()
            .iter()
            .filter_map(|selection| {
                self.multi_buffer
                    .read(cx)
                    .location_for_range(selection.range())
            })
            .collect::<Vec<_>>();
        if !locations.is_empty() {
            cx.emit(EditorEvent::OpenExcerptsRequested {
                locations,
                split: false,
            });
        }
    }

    /// 生效的换行模式：SingleLine 恒为不换行——单行输入只有一行视口，换行会把文本切到可见范围外（光标跟随的是换行后的显示行，前段文字整体不可见）；
    /// 其余模式覆盖优先，否则跟随全局设置。
    pub(crate) fn soft_wrap(&self) -> SoftWrap {
        if self.mode == EditorMode::SingleLine {
            return SoftWrap::None;
        }
        self.soft_wrap_override.unwrap_or(self.soft_wrap)
    }

    pub(crate) fn preferred_line_length(&self) -> usize {
        self.preferred_line_length
    }

    pub(crate) fn mode(&self) -> &EditorMode {
        &self.mode
    }

    /// 由渲染层每帧调用：把文本区宽度与当前字体交给 DisplayMap，变化时重建换行点。
    pub(crate) fn set_wrap_width(
        &mut self,
        wrap_width: Option<gpui::Pixels>,
        font: gpui::Font,
        font_size: gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> bool {
        let text_system = cx.text_system().clone();
        let changed = self.display_map.update(cx, |map, cx| {
            map.set_wrap_width(wrap_width, font, font_size, &text_system, cx)
        });
        if changed {
            self.advance_snapshots(cx);
        }
        changed
    }

    pub fn file_path(&self, cx: &App) -> Option<PathBuf> {
        self.multi_buffer.read(cx).file_path(cx)
    }

    pub fn language_name(&self, cx: &App) -> Option<&'static str> {
        // 组合文档按光标所在 excerpt 的源语言显示。
        let offset = self.resolved_selections(cx).primary().head();
        self.multi_buffer.read(cx).language_at(offset, cx)
    }

    pub fn set_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.multi_buffer
            .update(cx, |buffer, cx| buffer.set_file_path(path, cx));
        cx.emit(EditorEvent::PathChanged);
    }

    /// 为当前编辑器文档安装 Git diff projection。
    ///
    /// `None` 是加载态（新 diff 尚未算完）：保留现有 hunks 与用户展开状态，不再被中间空列表清空；展开状态按工作区文本跟踪区间跨刷新迁移。
    /// diff 状态与 excerpts projection 归属当前编辑器的 MultiBuffer 文档模型；
    /// 本方法只负责把外部 Git 基准更新提交给该模型并同步视图层状态。
    /// 返回 `true` 表示组合文档被重建；选区仍由源锚点解析，不随投影替换移动。
    /// 用给定文件列表整体替换 diff 投影；刷新等结构性变化的入口。
    pub fn set_diff_files(&mut self, files: Vec<DiffFile>, cx: &mut Context<Self>) -> bool {
        let rebuilt = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.set_diff_files(files, cx));
        self.reset_after_diff_injection(rebuilt, cx);
        rebuilt
    }

    /// 按路径增量挂接一个文件的 diff。
    pub fn add_diff(&mut self, file: DiffFile, cx: &mut Context<Self>) -> bool {
        let rebuilt = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.add_diff(file, cx));
        self.reset_after_diff_injection(rebuilt, cx);
        rebuilt
    }

    /// 移除指定显示路径的 diff。
    pub fn remove_diff(&mut self, path: &std::path::Path, cx: &mut Context<Self>) -> bool {
        let rebuilt = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.remove_diff(path, cx));
        self.reset_after_diff_injection(rebuilt, cx);
        rebuilt
    }

    /// 清除全部 diff。
    pub fn clear_diffs(&mut self, cx: &mut Context<Self>) -> bool {
        let rebuilt = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.clear_diffs(cx));
        self.reset_after_diff_injection(rebuilt, cx);
        rebuilt
    }

    /// 当前已挂接 diff 的显示路径集合（按组合文档顺序）。
    pub fn diff_paths(&self, cx: &App) -> Vec<std::path::PathBuf> {
        self.multi_buffer.read(cx).diff_paths()
    }

    pub fn set_diff_hunk_delegate(
        &mut self,
        delegate: Option<Arc<dyn DiffHunkDelegate>>,
        cx: &mut Context<Self>,
    ) {
        self.diff_hunk_delegate = delegate;
        if self.diff_hunk_delegate.is_none() {
            self.hovered_diff_hunk = None;
        }
        cx.notify();
    }

    pub(crate) fn set_hovered_diff_hunk(&mut self, hunk: Option<usize>, cx: &mut Context<Self>) {
        if self.hovered_diff_hunk != hunk {
            self.hovered_diff_hunk = hunk;
            cx.notify();
        }
    }

    /// 注入宿主拥有的文档内 hunk 装饰。
    ///
    /// Editor 只把锚点范围交给显示链；
    /// 显示坐标投影与视口查询归 DisplayMap，不再在 Editor 中建立并列的显示坐标装饰缓存。
    pub fn set_editor_hunks(&mut self, hunks: Vec<EditorHunk>, cx: &mut Context<Self>) {
        let hunks = Arc::from(hunks);
        self.display_map
            .update(cx, |map, cx| map.set_editor_hunks(hunks, cx));
        self.advance_snapshots(cx);
        cx.notify();
    }

    pub(crate) fn diff_hunk_delegate(&self) -> Option<Arc<dyn DiffHunkDelegate>> {
        self.diff_hunk_delegate.clone()
    }

    pub(crate) fn hovered_diff_hunk(&self) -> Option<usize> {
        self.hovered_diff_hunk
    }

    /// 设置新 hunk 的初始展开策略；
    /// 用户之后的显式展开/折叠不受投影刷新覆盖。
    pub fn set_diff_hunks_expanded_by_default(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.multi_buffer.update(cx, |buffer, cx| {
            buffer.set_diff_hunks_expanded_by_default(expanded, cx)
        });
        self.after_diff_expansion(cx);
    }

    /// base 版本变化后由宿主调用：重置展开状态（旧侧坐标空间失效时）。
    pub fn reset_diff_hunk_expansion_state(&mut self, cx: &mut Context<Self>) {
        self.multi_buffer
            .update(cx, |buffer, cx| buffer.reset_diff_hunk_expansion_state(cx));
        self.after_diff_expansion(cx);
    }

    /// 与当前组合文档版本匹配的显示坐标 hunks。
    pub fn diff_hunks(&self, cx: &App) -> Vec<DisplayHunk> {
        self.multi_buffer.read(cx).diff_hunks()
    }

    /// 每个 hunk 在组合文档中的旧侧显示行范围（与 diff_hunks 同门控）。
    pub fn diff_hunk_old_ranges(&self, cx: &App) -> Vec<Option<Range<usize>>> {
        self.multi_buffer.read(cx).diff_hunk_old_ranges()
    }

    /// 与 diff_hunks 平行的展开标志（渲染层按显示 hunk 索引查询）。
    pub fn diff_hunk_expanded(&self, cx: &App) -> Vec<bool> {
        self.multi_buffer.read(cx).diff_hunk_expanded()
    }

    /// 与 diff_hunks 平行的词级变化片段（组合文档字节范围 + 新增/删除色）。
    pub fn diff_hunk_word_diffs(&self, cx: &App) -> Vec<WordDiffs> {
        self.multi_buffer.read(cx).diff_hunk_word_diffs()
    }

    /// 按显示 hunk 索引切换展开/折叠（渲染层点击入口）。
    pub fn toggle_diff_hunk_at(&mut self, display_index: usize, cx: &mut Context<Self>) {
        self.multi_buffer.update(cx, |buffer, cx| {
            buffer.toggle_diff_hunk_at(display_index, cx)
        });
        self.after_diff_expansion(cx);
    }

    /// 显示 hunk 到源定位（hunk 操作与导航用）。
    pub fn buffer_diff_hunk_at(&self, display_index: usize, cx: &App) -> Option<DiffHunkSource> {
        self.multi_buffer
            .read(cx)
            .buffer_diff_hunk_at(display_index, cx)
    }

    /// 宿主注入/刷新整份 diff 投影后同步视图层状态。
    ///
    /// 选区由源锚点拥有，组合 excerpts 重建不会改变其源位置；
    /// 这里只同步显示快照，不得把选区重置到组合文档起点。
    fn reset_after_diff_injection(&mut self, rebuilt: bool, cx: &mut Context<Self>) {
        if rebuilt {
            self.advance_snapshots(cx);
        }
        cx.notify();
    }

    /// diff 展开/折叠重建后同步视图层状态：
    /// 结构刷新不改变源，源锚点选区自然存活——同步 DisplayMap 后按重建后快照解析即落到同一逻辑源位置，光标不会被重置到开头（与普通编辑器折叠不移动光标一致）。
    fn after_diff_expansion(&mut self, cx: &mut Context<Self>) {
        self.advance_snapshots(cx);
        cx.notify();
    }

    /// 注入宿主拥有的显式折叠候选。
    ///
    /// 显式范围优先于同一行的语法折叠建议；语法候选仍由显示层按行即时查询。
    pub fn insert_creases(
        &mut self,
        ranges: impl IntoIterator<Item = Range<MultiBufferAnchor>>,
        cx: &mut Context<Self>,
    ) -> Vec<ExplicitCreaseId> {
        let ids = self
            .display_map
            .update(cx, |map, cx| map.insert_creases(ranges, cx));
        if !ids.is_empty() {
            self.advance_snapshots(cx);
            cx.notify();
        }
        ids.into_iter().map(ExplicitCreaseId).collect()
    }

    /// 移除先前注入的显式折叠候选。
    pub fn remove_creases(
        &mut self,
        ids: impl IntoIterator<Item = ExplicitCreaseId>,
        cx: &mut Context<Self>,
    ) {
        let ids = ids.into_iter().map(|id| id.0).collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        self.display_map
            .update(cx, |map, cx| map.remove_creases(ids, cx));
        self.advance_snapshots(cx);
        cx.notify();
    }

    /// 折叠/展开指定逻辑行（crease 点击与 ToggleFold 命令的共享实现）。
    ///
    /// 该行是折叠入口行则展开覆盖它的折叠；否则若该行是可折叠范围起点则折叠整个范围。
    pub(crate) fn toggle_fold_at_line(&mut self, line: Line, cx: &mut Context<Self>) {
        let display_snapshot = self.display_snapshot(cx).clone();
        if display_snapshot.fold_anchor_lines().contains(&line) {
            let line_range =
                LineRange::new(line, Line::new(line.get() + 1)).expect("光标行 +1 应合法");
            if let Err(error) = self
                .display_map
                .update(cx, |map, cx| map.unfold_lines(line_range, cx))
            {
                cx.emit(EditorEvent::Error(format!("展开折叠失败：{error:#}")));
            }
        } else {
            let range = display_snapshot
                .crease_at_line(line)
                .map(|crease| crease.range().clone());
            if let Some(range) = range
                && let Err(error) = self
                    .display_map
                    .update(cx, |map, cx| map.fold_range(range, cx))
            {
                cx.emit(EditorEvent::Error(format!("折叠失败：{error:#}")));
            }
        }
        self.advance_snapshots(cx);
        cx.notify();
    }

    /// 按当前光标所在显示行切换折叠。
    ///
    /// 已折叠时，入口文本、占位符与闭合尾段属于同一显示行，从其中任意位置触发都展开该行的折叠；
    /// 未折叠时，折叠包含光标逻辑行的最内层范围。
    fn toggle_fold_at_cursor(&mut self, cx: &mut Context<Self>) {
        let head = self.resolved_selections(cx).primary().head();
        let display_snapshot = self.display_snapshot(cx).clone();
        let Ok(display_row) = display_snapshot
            .offset_to_display_point(head)
            .map(DisplayPoint::row)
        else {
            return;
        };
        let folded_anchor_lines = display_snapshot
            .fold_anchor_lines()
            .into_iter()
            .filter(|line| display_snapshot.line_to_display_row(*line) == Some(display_row))
            .collect::<Vec<_>>();

        if !folded_anchor_lines.is_empty() {
            for line in folded_anchor_lines {
                let line_range =
                    LineRange::new(line, Line::new(line.get() + 1)).expect("折叠入口行 +1 应合法");
                if let Err(error) = self
                    .display_map
                    .update(cx, |map, cx| map.unfold_lines(line_range, cx))
                {
                    cx.emit(EditorEvent::Error(format!("展开折叠失败：{error:#}")));
                }
            }
            self.advance_snapshots(cx);
            cx.notify();
            return;
        }

        let snapshot = self.render_snapshot(cx);
        let Ok(head_line) = snapshot.byte_to_line(head) else {
            return;
        };
        let range = display_snapshot
            .crease_containing_line(head_line)
            .map(|crease| crease.range().clone());

        if let Some(range) = range
            && let Err(error) = self
                .display_map
                .update(cx, |map, cx| map.fold_range(range, cx))
        {
            cx.emit(EditorEvent::Error(format!("折叠失败：{error:#}")));
        }
        self.advance_snapshots(cx);
        cx.notify();
    }

    /// 读取整份组合文本的只读边界；供 Input 契约等外部取回文本使用。
    ///
    /// 会物化整份文本，编辑与显示热路径不得调用；只需判空时用快照的 len_bytes()。
    pub fn text(&self, cx: &App) -> String {
        String::from_utf8(self.display_snapshot(cx).buffer_snapshot().text_bytes())
            .expect("组合文本必须是合法 UTF-8")
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        self.multi_buffer.read(cx).is_dirty(cx)
    }

    /// 设置空 buffer 时显示的提示文本。
    ///
    /// 文本放进独立 DisplayMap：渲染层在空 buffer 时把它的快照接入行管线，
    /// 折行/行高/滚动与真实文本一致；空文本清除 placeholder。
    pub fn set_placeholder_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        self.placeholder_display_map = if text.is_empty() {
            None
        } else {
            let buffer = Buffer::from_text(text, BufferConfig::default())
                .expect("placeholder Buffer 应能创建");
            let tab_width = self
                .multi_buffer
                .update(cx, |buffer, cx| buffer.snapshot(cx))
                .language_settings()
                .tab
                .tab_width;
            Some(cx.new(|cx| {
                let mut map = DisplayMap::new(buffer.snapshot(), cx);
                map.set_tab_width(tab_width, cx);
                map
            }))
        };
    }

    /// 空 buffer 且有 placeholder 时返回其快照（渲染层行数据源替换用）。
    ///
    /// 判空走当前快照的字节长度，不能在每帧渲染路径上物化整份组合文本。
    pub(super) fn placeholder_snapshot_if_empty(&self, cx: &App) -> Option<DisplaySnapshot> {
        if self
            .display_snapshot(cx)
            .buffer_snapshot()
            .len_bytes()
            .get()
            != 0
        {
            return None;
        }
        self.placeholder_display_map
            .as_ref()
            .map(|map| map.read(cx).cached_snapshot())
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.is_read_only(cx) {
            return;
        }
        self.composition = None;
        let before_selections = self.resolved_selections(cx);
        let end = self.display_snapshot(cx).buffer_snapshot().len_bytes();
        let targets = SelectionSet::new(vec![Selection::new(MultiBufferOffset::ZERO, end)]);
        let text = if self.mode == EditorMode::SingleLine {
            text.replace(['\r', '\n'], "")
        } else {
            text.to_owned()
        };
        let metadata = edit_metadata("设置文本");
        let _ = self.change_with_after(before_selections, metadata.clone(), cx, |buffer| {
            replace_selections(buffer, &targets, &text)
        });
    }

    /// 将单个选择区设置为给定的 UTF-8 字节范围。
    pub fn select_byte_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let end = self.display_snapshot(cx).buffer_snapshot().len_bytes();
        assert!(range.start <= range.end && MultiBufferOffset::new(range.end) <= end);
        self.change_selections(
            SelectionSet::new(vec![Selection::new(
                MultiBufferOffset::new(range.start),
                MultiBufferOffset::new(range.end),
            )]),
            cx,
        );
    }

    /// 跳转到 0-indexed 逻辑行列，并把目标固定在视口顶部下方。
    /// 列按 Unicode scalar value 计数，与 zcv-text Position 坐标一致。
    pub fn navigate_to_line_column(
        &mut self,
        line: usize,
        column: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let snapshot = self.render_snapshot(cx);
        let Ok(offset) =
            snapshot.position_to_byte(Position::new(Line::new(line), LogicalColumn::new(column)))
        else {
            return false;
        };
        self.change_selections(SelectionSet::caret(offset), cx);
        self.request_scroll_to_top(NAVIGATION_TOP_OFFSET, cx);
        true
    }

    /// 选区变更样板：结束组合会话、重锚定选区、请求自动滚动并清空 IME 布局缓存。
    pub(super) fn change_selections(&mut self, selections: SelectionSet, cx: &mut Context<Self>) {
        self.structured_selection_history.clear();
        self.apply_selection_change(selections, cx);
    }

    fn apply_selection_change(&mut self, selections: SelectionSet, cx: &mut Context<Self>) {
        self.composition = None;
        self.set_selections_without_clearing_structured_history(selections, cx);
        self.request_autoscroll(cx);
        self.input_layout = None;
        cx.notify();
    }

    pub(crate) fn render_snapshot(&self, cx: &App) -> MultiBufferSnapshot {
        self.display_snapshot(cx).buffer_snapshot().clone()
    }

    pub(super) fn text_snapshot(&self, cx: &App) -> MultiBufferSnapshot {
        self.display_snapshot(cx).buffer_snapshot().clone()
    }

    /// 当前显示快照；唯一权威由 `display_map` 实体持有，Editor 不缓存第二份。
    pub(crate) fn display_snapshot(&self, cx: &App) -> DisplaySnapshot {
        self.display_map.read(cx).cached_snapshot()
    }

    /// 构造一帧渲染使用的只读快照；渲染层从它读取显示状态与滚动/显示选项。
    ///
    /// 聚焦状态进入快照；光标可见性由渲染层用该状态与闪烁态共同决定。
    pub(crate) fn snapshot(&self, window: &Window, cx: &App) -> EditorSnapshot {
        EditorSnapshot {
            display_snapshot: self.display_snapshot(cx),
            placeholder_display_snapshot: self.placeholder_snapshot_if_empty(cx),
            mode: self.mode().clone(),
            shows_gutter: self.shows_gutter(),
            soft_wrap: self.soft_wrap(),
            preferred_line_length: self.preferred_line_length(),
            scroll_anchor: self.scroll_manager.anchor(),
            scroll_offset: self.scroll_manager.offset(),
            is_focused: window.is_window_active() && self.focus.is_focused(window),
        }
    }

    /// 搜索命中的滚动条标记是否可见；与 Zed 一致，仅单文档编辑器消费。
    pub(crate) fn shows_search_scrollbar_markers(&self, cx: &App) -> bool {
        self.multi_buffer.read(cx).singleton_source().is_some()
    }

    /// 当前缓存的滚动条标记分组（index 0 = diff，index 1 = search）。
    pub(crate) fn scrollbar_marker_groups(&self) -> [Option<Arc<[ScrollbarMarker]>>; 2] {
        self.scrollbar_marker_state.marker_groups.clone()
    }

    /// 刷新滚动条慢标记缓存；几何计算在后台执行，渲染帧只读缓存。
    pub(crate) fn refresh_scrollbar_markers(
        &mut self,
        display_snapshot: DisplaySnapshot,
        track_bounds: Bounds<Pixels>,
        scroll_per_pixel: f32,
        line_height: Pixels,
        cx: &mut Context<Self>,
    ) {
        if !self
            .scrollbar_marker_state
            .should_refresh(track_bounds.size)
        {
            return;
        }
        let is_singleton = self.shows_search_scrollbar_markers(cx);
        let task = cx.background_spawn(async move {
            display_snapshot.scrollbar_marker_groups(
                track_bounds,
                scroll_per_pixel,
                line_height,
                is_singleton,
            )
        });
        let handle = cx.spawn(async move |this, cx| {
            let groups = task.await;
            this.update(cx, |editor, cx| {
                editor
                    .scrollbar_marker_state
                    .finish_refresh(track_bounds.size, groups);
                cx.notify();
            })
            .ok();
        });
        self.scrollbar_marker_state.begin_refresh(handle);
    }

    pub(super) fn longest_line_width(
        &mut self,
        row: DisplayRow,
        font: gpui::Font,
        font_size: Pixels,
        window: &mut Window,
        cx: &App,
    ) -> Pixels {
        let snapshot = self.display_snapshot(cx).clone();
        let font_id = window.text_system().resolve_font(&font);
        if let Some(cache) = &self.line_width_cache
            && cache.version == snapshot.buffer_snapshot().version()
            && cache.row == row
            && cache.font_id == font_id
            && cache.font_size == font_size
        {
            return cache.width;
        }
        let width = layout_line_width(&snapshot, row, &font, font_size, window);
        self.line_width_cache = Some(LineWidthCache {
            version: snapshot.buffer_snapshot().version(),
            row,
            font_id,
            font_size,
            width,
        });
        width
    }

    pub(super) fn matching_bracket_pair(&mut self, cx: &App) -> Option<BracketPair> {
        // 任一选区非空时都不显示匹配括号高亮，避免把括号强调误认成选区的一部分；
        // 判断在缓存之前，选区状态变化不会命中陈旧缓存。
        let selections = self.resolved_selections(cx);
        if selections
            .as_slice()
            .iter()
            .any(|selection| !selection.is_caret())
        {
            return None;
        }
        let display_snapshot = self.display_snapshot(cx);
        let snapshot = display_snapshot.buffer_snapshot();
        let caret = selections.primary().head();
        let buffer_version = snapshot.version();
        let metadata_version = snapshot.metadata_version();
        if let Some((cached_caret, cached_buffer, cached_metadata, cached)) =
            &self.bracket_pair_cache
            && *cached_caret == caret
            && *cached_buffer == buffer_version
            && *cached_metadata == metadata_version
        {
            return cached.clone();
        }
        let caret_offset = caret.get();
        let result = self
            .display_snapshot(cx)
            .buffer_snapshot()
            .bracket_pairs_at(caret)
            .into_iter()
            .find(|pair| {
                [
                    pair.open.start,
                    pair.open.end,
                    pair.close.start,
                    pair.close.end,
                ]
                .contains(&caret_offset)
            });
        self.bracket_pair_cache = Some((caret, buffer_version, metadata_version, result.clone()));
        result
    }

    pub(crate) fn selections(&self, cx: &App) -> SelectionSet {
        self.resolved_selections(cx)
    }

    /// 把 offset 版选区集合重锚定到当前显示快照版本。
    pub(crate) fn set_selections(&mut self, selections: SelectionSet, cx: &App) {
        self.structured_selection_history.clear();
        self.set_selections_without_clearing_structured_history(selections, cx);
    }

    fn set_selections_without_clearing_structured_history(
        &mut self,
        selections: SelectionSet,
        cx: &App,
    ) {
        // 任何普通选区替换都会终止 pending selection，避免旧鼠标锚点在之后复活。
        self.pending_selection = None;
        self.set_pending_selection(selections, cx);
    }

    /// 更新 pending selection 显示出的当前选区。
    /// 只有 begin/update selection 可以调用这个入口。
    fn set_pending_selection(&mut self, selections: SelectionSet, cx: &App) {
        self.selections = selections.anchored(self.display_snapshot(cx).buffer_snapshot());
    }

    /// 按当前派生快照把源锚点选区解析为投影 offset 版选区集合。
    fn resolved_selections(&self, cx: &App) -> SelectionSet {
        self.selections
            .resolve(self.display_snapshot(cx).buffer_snapshot())
    }

    /// 光标位置的 "行:列" 文本，行和列均从 1 开始计数。
    /// 组合文档（多文件编辑器）按光标所在 excerpt 映射回源文件内的真实行列。
    pub fn cursor_text(&self, cx: &App) -> String {
        let head = self.resolved_selections(cx).primary().head();
        let multi_snapshot = self.display_snapshot(cx).buffer_snapshot().clone();
        // 无片段的空组合文档：光标没有归属的源文件，不显示行列。
        if multi_snapshot.excerpts().next().is_none() {
            return String::new();
        }
        let Ok(point) = multi_snapshot.byte_to_position(head) else {
            return String::new();
        };
        // 行号：excerpt 映射回的源行已是 1 起始（source_start_line 约定，与 gutter/悬浮标题一致）；
        // 单文件文档的组合行是 0 起始，需转 1 起始显示。
        let line = match multi_snapshot.excerpt_for_output_line(point.line().get()) {
            // Deleted 片段的内容来自 Git 修订文本：光标停留在修订文本上，显示修订文本中的行列（该行在修订版本中的真实行号）。
            Some(excerpt) if excerpt.diff_kind() == Some(ExcerptDiffKind::Deleted) => {
                excerpt.source_start_line() + point.line().get() - excerpt.output_start_line()
            }
            // 片段内输出文本与源文本一致，列号直接沿用。
            Some(excerpt) => {
                let Some(source_line) = excerpt.source_line_for_output_line(point.line().get())
                else {
                    return String::new();
                };
                source_line
            }
            // 单文件文档：组合坐标即源坐标。
            None => point.line().get() + 1,
        };
        let column = point.column().get() + 1;
        format!("{line}:{column}")
    }

    pub(super) fn presentation(&self, cx: &App) -> EditorPresentation {
        EditorPresentation::new(
            self.display_snapshot(cx).buffer_snapshot(),
            self.composition.as_ref(),
        )
        .with_dimmed_ranges(self.local_rename_ranges())
    }

    pub(super) fn shows_gutter(&self) -> bool {
        self.mode == EditorMode::Full
    }

    pub(super) fn active_lines_in_range(
        &self,
        visible: Option<&Range<Line>>,
        cx: &App,
    ) -> Vec<Line> {
        let Some(visible) = visible else {
            return Vec::new();
        };
        let display_snapshot = self.display_snapshot(cx);
        let snapshot = display_snapshot.buffer_snapshot();
        let mut lines = std::collections::BTreeSet::new();
        for selection in self.resolved_selections(cx).as_slice() {
            let range = selection.range();
            let Ok(start) = snapshot.byte_to_line(range.start()) else {
                continue;
            };
            let Ok(mut end) = snapshot.byte_to_line(range.end()) else {
                continue;
            };
            if !range.is_empty()
                && end > start
                && snapshot.line_start_byte(end).ok() == Some(range.end())
            {
                end = Line::new(end.get() - 1);
            }
            let start = start.max(visible.start);
            let end = end.min(Line::new(visible.end.get().saturating_sub(1)));
            if start <= end {
                lines.extend((start.get()..=end.get()).map(Line::new));
            }
        }
        lines.into_iter().collect()
    }

    pub(super) fn scroll_anchor(&self) -> DisplayPoint {
        self.scroll_manager.anchor()
    }

    pub(super) fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll_manager.offset()
    }

    pub(super) fn longest_display_row(&self, cx: &App) -> DisplayRow {
        self.display_map.read(cx).longest_measured_row()
    }

    pub(super) fn measure_display_rows(
        &mut self,
        start: DisplayRow,
        line_count: usize,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self
            .display_map
            .update(cx, |map, cx| map.measure_rows(start, line_count, cx))
        {
            eprintln!("Editor 测量显示行失败：{error}");
        }
    }

    pub(super) fn select_line(&mut self, line: Line, extend: bool, cx: &App) {
        let snapshot = self.render_snapshot(cx);
        let Ok(start) = snapshot.line_start_byte(line) else {
            return;
        };
        let end = snapshot
            .line_start_byte(Line::new(line.get() + 1))
            .unwrap_or_else(|_| snapshot.len_bytes());
        let selection = if extend {
            let current = *self
                .selections
                .resolve(self.display_snapshot(cx).buffer_snapshot())
                .primary();
            if end <= current.start() {
                Selection::new(current.end(), start)
            } else if start >= current.end() {
                Selection::new(current.start(), end)
            } else {
                current
            }
        } else {
            Selection::new(start, end)
        };
        self.composition = None;
        self.set_selections(SelectionSet::new(vec![selection]), cx);
        self.request_autoscroll(cx);
    }

    /// 鼠标左键按下：按点击次数开始选区手势，并记录拖拽起点。
    ///
    /// 单击定位光标、双击选中词、三击选中整行、四击及以上全选；
    /// `extend`（Shift 按下）时按上次手势粒度扩展选区。
    pub(super) fn begin_selection(
        &mut self,
        display_point: DisplayPoint,
        click_count: usize,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        self.structured_selection_history.clear();
        let snapshot = self.display_snapshot(cx).buffer_snapshot().clone();
        let offset = if extend {
            let anchor = self.resolved_selections(cx).primary().tail();
            let Ok(left_offset) = self
                .display_snapshot(cx)
                .display_point_to_offset_with_bias(display_point, FoldBias::Left)
            else {
                return;
            };
            let bias = if left_offset >= anchor {
                FoldBias::Right
            } else {
                FoldBias::Left
            };
            let Ok(offset) = self
                .display_snapshot(cx)
                .display_point_to_offset_with_bias(display_point, bias)
            else {
                return;
            };
            offset
        } else {
            let Ok(offset) = self
                .display_snapshot(cx)
                .display_point_to_offset_with_bias(display_point, FoldBias::Left)
            else {
                return;
            };
            offset
        };
        let Ok(char_offset) = snapshot.byte_to_char(offset) else {
            return;
        };
        // Shift 按下时按上次手势粒度提升点击次数：双击后 Shift+点按词扩展，三击后按行扩展。
        let click_count = if extend {
            click_count.max(match self.mouse_select_mode {
                MouseSelectMode::Character => 1,
                MouseSelectMode::Word(_) => 2,
                MouseSelectMode::Line(_) => 3,
                MouseSelectMode::All => 4,
            })
        } else {
            click_count
        };

        // 本次点击粒度对应的候选范围；锚定后跨编辑仍按当前快照解析。
        let (start, end, mode) = match click_count {
            1 => (offset, offset, MouseSelectMode::Character),
            2 => {
                let Ok((word_start, word_end)) = snapshot.surrounding_word(char_offset) else {
                    return;
                };
                let Ok(word_start) = snapshot.char_to_byte(word_start) else {
                    return;
                };
                let Ok(word_end) = snapshot.char_to_byte(word_end) else {
                    return;
                };
                (
                    word_start,
                    word_end,
                    MouseSelectMode::Word(
                        snapshot.anchor_at(word_start, Affinity::Before)
                            ..snapshot.anchor_at(word_end, Affinity::After),
                    ),
                )
            }
            3 => {
                let Ok(line) = snapshot.byte_to_line(offset) else {
                    return;
                };
                let Ok(line_start) = snapshot.line_start_byte(line) else {
                    return;
                };
                let line_end = snapshot
                    .line_start_byte(Line::new(line.get() + 1))
                    .unwrap_or(snapshot.len_bytes());
                (
                    line_start,
                    line_end,
                    MouseSelectMode::Line(
                        snapshot.anchor_at(line_start, Affinity::Before)
                            ..snapshot.anchor_at(line_end, Affinity::After),
                    ),
                )
            }
            _ => (
                MultiBufferOffset::ZERO,
                snapshot.len_bytes(),
                MouseSelectMode::All,
            ),
        };

        // Shift+点击：以上次选区锚点为固定端，按点击位置向两侧扩展；点击范围覆盖锚点时整段纳入。
        let selection = if extend {
            let tail = self.resolved_selections(cx).primary().tail();
            let mut start = start;
            let mut end = end;
            let mut reversed = false;
            if start > tail {
                start = tail;
            }
            if end < tail {
                end = tail;
                reversed = true;
            }
            Selection::new(
                if reversed { end } else { start },
                if reversed { start } else { end },
            )
        } else {
            self.mouse_select_mode = mode.clone();
            Selection::new(start, end)
        };

        self.composition = None;
        self.set_pending_selection(SelectionSet::new(vec![selection]), cx);
        self.request_autoscroll(cx);
        self.input_layout = None;
        self.pending_selection = Some(PendingSelection {
            anchor: snapshot.anchor_at(offset, Affinity::After),
            mode,
        });
        cx.notify();
    }

    /// 鼠标拖动：按按下时的粒度把选区活动端更新到当前位置。
    ///
    /// 词/行粒度下按整词/整行边界吸附，避免半词截断。
    pub(super) fn update_selection(&mut self, display_point: DisplayPoint, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_selection.clone() else {
            return;
        };
        let snapshot = self.display_snapshot(cx).buffer_snapshot().clone();
        // 拖动端点锚点版本已被 reset / 基线替换淘汰时无法继续拖动，显式放弃本次更新。
        let Some(pending_anchor) = snapshot.resolve_anchor(&pending.anchor) else {
            return;
        };
        let resolve_original = |range: &Range<MultiBufferAnchor>| {
            Some((
                snapshot.resolve_anchor(&range.start)?,
                snapshot.resolve_anchor(&range.end)?,
            ))
        };
        let Ok(left_offset) = self
            .display_snapshot(cx)
            .display_point_to_offset_with_bias(display_point, FoldBias::Left)
        else {
            return;
        };
        let fold_bias = if left_offset >= pending_anchor {
            FoldBias::Right
        } else {
            FoldBias::Left
        };
        let Ok(offset) = self
            .display_snapshot(cx)
            .display_point_to_offset_with_bias(display_point, fold_bias)
        else {
            return;
        };
        let Ok(char_offset) = snapshot.byte_to_char(offset) else {
            return;
        };
        let (head, tail) = match pending.mode {
            MouseSelectMode::Character => (offset, pending_anchor),
            MouseSelectMode::Word(original_range) => {
                let Some((original_start, original_end)) = resolve_original(&original_range) else {
                    // 原词锚点版本已被替换：退回按当前偏移落点。
                    return;
                };
                // 光标仍在词内（或落在原词范围内）时按整词边界吸附，head 取点击侧的词端。
                let inside = snapshot.is_inside_word(char_offset).unwrap_or(false)
                    || (original_start..original_end).contains(&offset);
                let head = if inside {
                    let Ok((word_start, word_end)) = snapshot.surrounding_word(char_offset) else {
                        return;
                    };
                    let Ok(word_start) = snapshot.char_to_byte(word_start) else {
                        return;
                    };
                    let Ok(word_end) = snapshot.char_to_byte(word_end) else {
                        return;
                    };
                    if word_start < original_start {
                        word_start
                    } else {
                        word_end
                    }
                } else {
                    offset
                };
                // 活动端在原词左侧时锚定原词右端，否则锚定左端。
                if head <= original_start {
                    (head, original_end)
                } else {
                    (head, original_start)
                }
            }
            MouseSelectMode::Line(original_range) => {
                let Some((original_start, original_end)) = resolve_original(&original_range) else {
                    // 原行锚点版本已被替换：退回按当前偏移落点。
                    return;
                };
                // 行粒度：head 所在整行纳入（含行尾换行符）。
                let Ok(line) = snapshot.byte_to_line(offset) else {
                    return;
                };
                let Ok(line_start) = snapshot.line_start_byte(line) else {
                    return;
                };
                let next_line_start = snapshot
                    .line_start_byte(Line::new(line.get() + 1))
                    .unwrap_or(snapshot.len_bytes());
                let head = if line_start < original_start {
                    line_start
                } else {
                    next_line_start
                };
                if head <= original_start {
                    (head, original_end)
                } else {
                    (head, original_start)
                }
            }
            MouseSelectMode::All => return,
        };
        self.composition = None;
        self.set_pending_selection(SelectionSet::new(vec![Selection::new(tail, head)]), cx);
        self.input_layout = None;
        cx.notify();
    }

    /// 鼠标松开：结束选区手势，选区已随拖动落定。
    pub(super) fn end_selection(&mut self) {
        self.pending_selection = None;
    }

    /// 编辑器自身是否正在拖拽选区手势（拖拽滚动的生效守卫：`dragging` 事件是窗口级的，其他面板（如终端）拖拽时编辑器不应滚动）。
    pub(super) fn has_pending_selection(&self) -> bool {
        self.pending_selection.is_some()
    }

    pub(super) fn set_ime_caret_geometry(
        &mut self,
        element_bounds: Bounds<Pixels>,
        caret_bounds: Option<Bounds<Pixels>>,
    ) {
        let Some(caret_bounds) = caret_bounds else {
            return;
        };
        self.pixel_position_of_newest_cursor = Some(point(
            caret_bounds.origin.x - element_bounds.origin.x,
            caret_bounds.origin.y - element_bounds.origin.y,
        ));
        self.last_bounds = Some(element_bounds);
        self.last_line_height = Some(caret_bounds.size.height);
    }

    pub(super) fn prepare_scroll_viewport(
        &mut self,
        viewport_size: gpui::Size<Pixels>,
        content_width: Pixels,
        line_height: Pixels,
        top_inset: Pixels,
        cx: &App,
    ) {
        let display_snapshot = self.display_snapshot(cx).clone();
        self.scroll_manager.update_viewport(
            ScrollViewport::new(
                display_snapshot.line_count(),
                viewport_size.width,
                viewport_size.height,
                content_width,
                line_height,
                top_inset,
            ),
            &display_snapshot,
        );
    }

    pub(super) fn scroll_by(&mut self, delta: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let display_snapshot = self.display_snapshot(cx).clone();
        if self.scroll_manager.scroll_by(delta, &display_snapshot) {
            self.input_layout = None;
            cx.notify();
            true
        } else {
            false
        }
    }

    /// 布局前消费待自动滚动点并应用垂直部分（见 `ScrollManager::apply_pending_autoscroll_vertical`）。
    ///
    /// 目标显示点按当前布局快照解析：软换行宽度在此帧已确定，行号与最终布局一致，避免导航请求在换行重排前固化错误的目标行。
    pub(super) fn apply_pending_autoscroll_vertical(&mut self, cx: &App) -> bool {
        let display_snapshot = self.display_snapshot(cx).clone();
        self.scroll_manager
            .apply_pending_autoscroll_vertical(&display_snapshot)
    }

    /// 布局后做水平自动滚动钳制（见 `ScrollManager::complete_autoscroll_horizontal`）。
    pub(super) fn complete_autoscroll_horizontal(
        &mut self,
        caret_left: Option<Pixels>,
        caret_right: Option<Pixels>,
    ) -> bool {
        self.scroll_manager
            .complete_autoscroll_horizontal(caret_left, caret_right)
    }

    /// 可见区顶部滚动量（像素）。
    pub(super) fn scroll_top(&self) -> Pixels {
        self.scroll_manager.scroll_top()
    }

    /// 可滚动上界（像素）。
    pub(super) fn max_scroll_top(&self) -> Pixels {
        self.scroll_manager.max_scroll_top()
    }

    /// 绝对滚动到指定顶部位置（滚动轴拖动/跳页入口）。
    pub(super) fn scroll_to(&mut self, scroll_top: Pixels, cx: &mut Context<Self>) -> bool {
        let display_snapshot = self.display_snapshot(cx).clone();
        if self.scroll_manager.scroll_to(scroll_top, &display_snapshot) {
            self.input_layout = None;
            cx.notify();
            true
        } else {
            false
        }
    }

    /// 滚动轴 thumb 当前三态。
    pub(super) fn scrollbar_thumb_state(&self) -> ScrollbarThumbState {
        self.scroll_manager.thumb_state()
    }

    /// 置滚动轴 thumb 悬停态。
    pub(super) fn set_scrollbar_thumb_hovered(&mut self, cx: &mut Context<Self>) {
        if self.scroll_manager.set_thumb_hovered() {
            cx.notify();
        }
    }

    /// 置滚动轴 thumb 拖动态。
    pub(super) fn set_scrollbar_thumb_dragged(&mut self, cx: &mut Context<Self>) {
        if self.scroll_manager.set_thumb_dragged() {
            cx.notify();
        }
    }

    /// 复位滚动轴 thumb 为 Idle。
    pub(super) fn reset_scrollbar_thumb_state(&mut self, cx: &mut Context<Self>) {
        if self.scroll_manager.reset_thumb_state() {
            cx.notify();
        }
    }

    fn from_language_buffer(
        language_buffer: Entity<LanguageBuffer>,
        mode: EditorMode,
        cx: &mut Context<Self>,
    ) -> Self {
        let multi_buffer = cx.new(|cx| MultiBuffer::singleton(language_buffer, cx));
        Self::new(multi_buffer, mode, cx)
    }

    fn new(multi_buffer: Entity<MultiBuffer>, mode: EditorMode, cx: &mut Context<Self>) -> Self {
        // 在一次底层 Buffer 更新中建立订阅并取得同版本组合快照，关闭初始化期间的漏读窗口。
        let (multi_buffer_subscription, snapshot) =
            multi_buffer.update(cx, |buffer, cx| buffer.subscribe_and_snapshot(cx));
        let last_dirty = multi_buffer.read(cx).is_dirty(cx);
        let display_map = cx.new(|cx| {
            let mut map = DisplayMap::new(snapshot.clone(), cx);
            map.set_multi_buffer(multi_buffer.clone(), multi_buffer_subscription, cx);
            // Tab 宽度按 buffer/language 解析（对齐 Zed LanguageSettings），不再读全局设置。
            map.set_tab_width(snapshot.language_settings().tab.tab_width, cx);
            map
        });
        let display_snapshot = display_map.update(cx, |map, cx| map.snapshot(cx));
        let initial_selections =
            SelectionSet::default().anchored(display_snapshot.buffer_snapshot());
        cx.subscribe(&multi_buffer, |editor, _, _, cx| {
            let multi_buffer = editor.multi_buffer.clone();
            let dirty = multi_buffer.read(cx).is_dirty(cx);
            if editor.last_dirty != dirty {
                editor.last_dirty = dirty;
                cx.emit(EditorEvent::DirtyChanged);
            }
            editor.advance_snapshots(cx);
            cx.notify();
        })
        .detach();
        let blink_manager = cx.new(|_| BlinkManager::new());
        cx.observe(&blink_manager, |_, _, cx| cx.notify()).detach();

        // 换行模式默认来自全局设置，与编辑器模式无关；
        // UI 场景可用 set_soft_wrap_mode 覆盖（覆盖存在时设置变化不生效）。
        let settings = SettingsStore::try_get(cx);
        let this = Self {
            multi_buffer,
            last_dirty,
            display_map,
            mode,
            content_typography: false,
            placeholder_display_map: None,
            selections: initial_selections,
            selection_history: SelectionHistory::default(),
            structured_selection_history: Vec::new(),
            bracket_pair_cache: None,
            scroll_manager: ScrollManager::default(),
            diff_hunk_delegate: None,
            last_drag_autoscroll: Cell::new(Instant::now() - AUTOSCROLL_INTERVAL),
            search: None,
            composition: None,
            input_layout: None,
            pixel_position_of_newest_cursor: None,
            last_bounds: None,
            last_line_height: None,
            focus: cx.focus_handle(),
            blink_manager,
            blink_manager_initialized: false,
            soft_wrap: settings
                .as_ref()
                .map_or(SoftWrap::default(), |settings| settings.soft_wrap.into()),
            soft_wrap_override: None,
            preferred_line_length: settings.map_or(80, |settings| settings.preferred_line_length),
            hovered_diff_hunk: None,
            mouse_select_mode: MouseSelectMode::Character,
            pending_selection: None,
            local_rename: None,
            autoclose_regions: Vec::new(),
            line_width_cache: None,
            scrollbar_marker_state: ScrollbarMarkerState::default(),
        };
        // DisplayMap 是显示投影的唯一权威：它变化后只重读快照刷新滚动模型并重绘，
        // Editor 不再持有可独立推进的第二份显示快照。
        cx.observe(&this.display_map, |editor, _, cx| {
            let snapshot = editor.display_snapshot(cx);
            editor.scroll_manager.refresh(&snapshot);
            cx.notify();
        })
        .detach();
        // 设置变化时自动跟随（覆盖场景除外）；编辑器在测试环境无 SettingsStore 时保持默认。
        cx.observe_global::<SettingsStore>(|editor, cx| {
            let Some(settings) = SettingsStore::try_get(cx) else {
                return;
            };
            if editor.soft_wrap_override.is_none() {
                editor.soft_wrap = settings.soft_wrap.into();
                editor.preferred_line_length = settings.preferred_line_length;
            }
            cx.notify();
        })
        .detach();
        this
    }

    /// 提交一次文本编辑事务的唯一入口。
    ///
    /// 会话模型：入口统一负责会话开启/提交（`start_transaction` / `end_transaction`）、Buffer 通知、编辑后选区锚点映射、SelectionHistory 记录、display_map 同步与搜索重搜（`apply_edit_outcome` 全链路）。
    /// `metadata` 是本次会话的历史元数据（描述与合并策略），由调用方决策并经 `MultiBuffer::edit` 透传到工作区源。
    /// 返回编辑结果供需要事务身份的调用方消费（如 IME 组合会话）；失败时错误已打印、选区已恢复，调用方只需处理自身特判状态。
    pub(super) fn change(
        &mut self,
        before_selections: SelectionSet,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut EditPlan<'_>) -> TextResult<EditOutcome>,
    ) -> TextResult<EditOutcome> {
        let (node_id, before_snapshot, outcome) =
            self.commit_session(before_selections, metadata, cx, f)?;
        self.apply_edit_outcome(before_snapshot, node_id, outcome, cx)
    }

    /// 编辑后选区由闭包按编辑语义重算的变体（删除、剪切、行移动、输入等特判场景）。
    pub(super) fn change_with_after(
        &mut self,
        before_selections: SelectionSet,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut EditPlan<'_>) -> TextResult<(EditOutcome, SelectionSet)>,
    ) -> TextResult<(EditOutcome, SelectionSet)> {
        let (node_id, before_snapshot, outcome) =
            self.commit_session(before_selections, metadata, cx, f)?;
        self.apply_edit_outcome_with_after(before_snapshot, node_id, outcome, cx)
    }

    /// 需要读取提交后投影才能确定选区的编辑变体。
    ///
    /// 行移动等操作的目标行只在源 Buffer 提交后才是权威坐标；
    /// 这里保持「计划 → 直接写入 MultiBuffer → 读取当前快照 → 选区落位」的一条路径，不为此创建临时文本快照。
    pub(super) fn change_with_after_post<P>(
        &mut self,
        before_selections: SelectionSet,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
        plan: impl FnOnce(&mut EditPlan<'_>) -> TextResult<(EditOutcome, P)>,
        after: impl FnOnce(P, &MultiBufferSnapshot) -> TextResult<SelectionSet>,
    ) -> TextResult<(EditOutcome, SelectionSet)> {
        let (node_id, before_snapshot, (outcome, post_state)) =
            self.commit_session(before_selections, metadata, cx, plan)?;
        let snapshot = self.display_snapshot(cx).buffer_snapshot().clone();
        let after_selections = after(post_state, &snapshot)?;
        self.apply_edit_outcome_with_after(
            before_snapshot,
            node_id,
            (outcome, after_selections),
            cx,
        )
    }

    /// 会话化编辑的共享骨架：开启会话并记录 undo 选区（事务开始时记录）→ 闭包编辑（统一 Buffer 通知）→ 提交会话，返回 (节点身份, 编辑结果)。
    ///
    /// 会话元数据（描述与合并策略）由调用方决策，随 `MultiBuffer::edit` 透传到工作区源的历史；
    /// 编辑失败时结束空会话（不产生历史节点）、恢复编辑前选区并回传错误；合并进前节点时清理会话自身的孤儿选区记录。
    fn commit_session<T>(
        &mut self,
        before_selections: SelectionSet,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut EditPlan<'_>) -> TextResult<T>,
    ) -> TextResult<(Option<TransactionId>, MultiBufferSnapshot, T)> {
        let operation = metadata.description().unwrap_or("编辑").to_owned();
        // 编辑前快照是本次事务的局部输入，不进入 Editor 长期状态。
        let before_snapshot = self.display_snapshot(cx).buffer_snapshot().clone();
        let session_id = self.start_transaction(cx)?;
        let mut plan = EditPlan::new(&before_snapshot);
        let outcome = match f(&mut plan) {
            Ok(outcome) => outcome,
            Err(error) => {
                self.end_transaction(cx);
                cx.emit(EditorEvent::Error(format!("{operation}失败：{error:#}")));
                self.selections = before_selections.clone().anchored(&before_snapshot);
                return Err(error);
            }
        };
        let edits = plan.into_edits();
        // 编辑映射与提交可能失败（如命中只读 excerpt）：失败必须结束空会话并恢复编辑前选区，否则事务残留会阻塞后续所有编辑。
        let applied = (|| -> TextResult<()> {
            if edits.is_empty() {
                return Ok(());
            }
            self.multi_buffer
                .update(cx, |buffer, cx| buffer.edit(edits, metadata, cx))
        })();
        self.advance_snapshots(cx);
        if let Err(error) = applied {
            self.end_transaction(cx);
            cx.emit(EditorEvent::Error(format!("{operation}失败：{error:#}")));
            self.selections = before_selections.clone().anchored(&before_snapshot);
            return Err(error);
        }
        let node_id = self.end_transaction(cx);
        if node_id != Some(session_id) {
            self.selection_history.remove_transaction(session_id);
        }
        Ok((node_id, before_snapshot, outcome))
    }

    /// 开启编辑会话并记录 undo 选区。
    ///
    /// Editor 不嵌套会话：zcv-text 会话已开启时视为内部错误。
    fn start_transaction(&mut self, cx: &mut Context<Self>) -> TextResult<TransactionId> {
        let transaction_id = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.start_transaction(cx))?;
        // 历史记录存源锚点：撤销/重做后 diff 投影可异步重建，选区不依赖重建时机。
        self.selection_history
            .insert_transaction(transaction_id, self.selections.clone());
        Ok(transaction_id)
    }

    /// 提交编辑会话；会话内无编辑时返回 `None`（编辑失败路径）。
    fn end_transaction(&mut self, cx: &mut Context<Self>) -> Option<TransactionId> {
        self.multi_buffer
            .update(cx, |buffer, cx| buffer.end_transaction(cx))
    }

    /// 编辑事务结果落位：自动闭合区域推进、redo 选区记录与事件发布。
    ///
    /// 选区以源锚点为单一数据源，编辑后按当前快照解析即自动跟随，不再经 PositionMap 重映射。
    /// 只有 `end_transaction` 返回真实事务身份时才发布 `Edited`；空事务只收尾不发布。
    fn apply_edit_outcome(
        &mut self,
        before_snapshot: MultiBufferSnapshot,
        transaction_id: Option<TransactionId>,
        outcome: EditOutcome,
        cx: &mut Context<Self>,
    ) -> TextResult<EditOutcome> {
        if let Some(position_map) = outcome.position_map() {
            let new_version = self.display_snapshot(cx).buffer_snapshot().version();
            self.update_autoclose_regions_with(
                position_map,
                before_snapshot.version(),
                new_version,
            );
        }
        self.finish_transaction(transaction_id, cx);
        Ok(outcome)
    }

    /// 行移动等特判场景：编辑后选区由闭包按行语义重算（「编辑后、重建前」投影坐标），直接锚定落位。
    fn apply_edit_outcome_with_after(
        &mut self,
        before_snapshot: MultiBufferSnapshot,
        transaction_id: Option<TransactionId>,
        outcome: (EditOutcome, SelectionSet),
        cx: &mut Context<Self>,
    ) -> TextResult<(EditOutcome, SelectionSet)> {
        let (outcome, after_selections) = outcome;
        if let Some(position_map) = outcome.position_map() {
            let new_version = self.display_snapshot(cx).buffer_snapshot().version();
            self.update_autoclose_regions_with(
                position_map,
                before_snapshot.version(),
                new_version,
            );
        }
        // 编辑后投影坐标直接在当前快照锚定为源锚点；投影重建不改变源，随后解析即忠实落位。
        self.selections = after_selections.anchored(self.display_snapshot(cx).buffer_snapshot());
        self.finish_transaction(transaction_id, cx);
        Ok((outcome, self.resolved_selections(cx)))
    }

    /// 事务收尾：记录 redo 选区、结束编辑态，并在有真实事务身份时发布唯一编辑事件。
    fn finish_transaction(
        &mut self,
        transaction_id: Option<TransactionId>,
        cx: &mut Context<Self>,
    ) {
        if let Some(transaction_id) = transaction_id
            && let Some(transaction) = self.selection_history.transaction_mut(transaction_id)
        {
            // 事务结束时记录 redo 选区（源锚点）。
            transaction.set_redo(self.selections.clone());
        }
        self.finish_edit(cx);
        if let Some(transaction_id) = transaction_id {
            cx.emit(EditorEvent::Edited { transaction_id });
        }
    }

    fn finish_edit(&mut self, cx: &mut Context<Self>) {
        self.structured_selection_history.clear();
        self.pending_selection = None;
        self.request_autoscroll(cx);
        self.input_layout = None;
        self.blink_manager.update(cx, |blink, cx| {
            blink.pause_blinking(cx);
        });
        cx.notify();
    }

    /// 将自动闭合区域随一次文本变更推进到新版本。
    ///
    /// 区域版本与变更起点失配时整体清空（说明存在未走编辑入口的文本变更，陈旧区域坐标已不可信，继续保留会误触发跳过/删对）。
    fn update_autoclose_regions_with(
        &mut self,
        position_map: &PositionMap,
        old_version: BufferVersion,
        new_version: BufferVersion,
    ) {
        let mut kept = Vec::with_capacity(self.autoclose_regions.len());
        for region in std::mem::take(&mut self.autoclose_regions) {
            if region.range.start.version() != old_version
                || region.range.end.version() != old_version
            {
                continue;
            }
            // 映射结果一律保留（Anchor 语义：删除内容不使锚失效）：
            // 区域锚在闭合符起点，闭合符是否存活由使用处的文本校验兜底。
            let range = region
                .range
                .start
                .map_through_position_map(new_version, position_map)
                .value()
                ..region
                    .range
                    .end
                    .map_through_position_map(new_version, position_map)
                    .value();
            kept.push(AutocloseRegion { range, ..region });
        }
        self.autoclose_regions = kept;
    }

    fn move_selections(
        &mut self,
        direction: MovementDirection,
        motion: impl Into<Motion>,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        let motion = motion.into();
        let selections = self.resolved_selections(cx);
        let primary_index = selections.primary_index();
        let outcome = selections
            .as_slice()
            .iter()
            .copied()
            .map(|selection| {
                // 非空选区按方向键移动时折叠到选区边缘：左右落在选区两端，上下从两端出发；
                // 词移动、行首尾等仍从 head 出发。
                let base = if extend || selection.is_caret() {
                    selection.head()
                } else {
                    match motion {
                        // 左右方向键与单行上下移动：从选区两端出发。
                        Motion::ByUnit(MovementUnit::Grapheme) | Motion::LineStep => {
                            match direction {
                                MovementDirection::Previous => selection.start(),
                                MovementDirection::Next => selection.end(),
                            }
                        }
                        // 翻页从选区底端出发（move_page_up/down 都基于 end）。
                        Motion::PageStep(_) => selection.end(),
                        _ => selection.head(),
                    }
                };
                // 垂直移动本次使用的目标列；移动后持久化到选区。
                let mut vertical_goal: Option<DisplayColumn> = None;
                let new_head = match motion {
                    Motion::ByUnit(unit) => {
                        // 左右方向键（grapheme 级）移动非空选区：折叠到选区端，不移动。
                        if !extend && !selection.is_caret() && unit == MovementUnit::Grapheme {
                            return Ok(Selection::caret(base).with_goal(None));
                        }
                        // 软换行模式下行首/行尾按显示行边界移动，其余单位走文本边界。
                        if unit == MovementUnit::LineEdge && self.display_snapshot(cx).is_wrapped()
                        {
                            let head = selection.head();
                            return match direction {
                                MovementDirection::Previous => self
                                    .display_snapshot(cx)
                                    .beginning_of_row(head)
                                    .map_err(|error| TextError::InvariantViolation {
                                        location: "Editor::move_selections",
                                        detail: error.to_string(),
                                    }),
                                MovementDirection::Next => self
                                    .display_snapshot(cx)
                                    .end_of_row(head)
                                    .map_err(|error| TextError::InvariantViolation {
                                        location: "Editor::move_selections",
                                        detail: error.to_string(),
                                    }),
                            }
                            .map(|new_head| {
                                // 行内水平移动清除垂直移动遗留的目标列。
                                (if extend {
                                    selection.with_head(new_head)
                                } else {
                                    Selection::caret(new_head)
                                })
                                .with_goal(None)
                            });
                        }
                        self.display_snapshot(cx)
                            .move_offset(base, direction, unit)?
                    }
                    Motion::LineStep | Motion::PageStep(_) => {
                        let row_step = match motion {
                            Motion::PageStep(row_step) => row_step,
                            _ => 1,
                        };
                        let point = self
                            .display_snapshot(cx)
                            .offset_to_display_point(base)
                            .map_err(|error| TextError::InvariantViolation {
                                location: "Editor::move_selections",
                                detail: error.to_string(),
                            })?;
                        // 目标列：优先使用持久化的 goal，否则从当前位置推导。
                        let goal = selection
                            .goal()
                            .map(DisplayColumn::new)
                            .unwrap_or(point.column());
                        vertical_goal = Some(goal);
                        let last_row = self.display_snapshot(cx).line_count().saturating_sub(1);
                        if direction == MovementDirection::Previous
                            && point.row() == DisplayRow::ZERO
                        {
                            return Ok(if extend {
                                selection
                                    .with_head(MultiBufferOffset::ZERO)
                                    .with_goal(Some(goal.get()))
                            } else {
                                Selection::caret(MultiBufferOffset::ZERO)
                                    .with_goal(Some(goal.get()))
                            });
                        }
                        if direction == MovementDirection::Next && point.row().get() >= last_row {
                            let new_head = self.display_snapshot(cx).buffer_snapshot().len_bytes();
                            return Ok(if extend {
                                selection.with_head(new_head).with_goal(Some(goal.get()))
                            } else {
                                Selection::caret(new_head).with_goal(Some(goal.get()))
                            });
                        }
                        let target_row = match direction {
                            MovementDirection::Previous => {
                                point.row().get().saturating_sub(row_step)
                            }
                            MovementDirection::Next => {
                                point.row().get().saturating_add(row_step).min(last_row)
                            }
                        };
                        let fold_bias = match direction {
                            MovementDirection::Previous => FoldBias::Left,
                            MovementDirection::Next => FoldBias::Right,
                        };
                        self.display_snapshot(cx)
                            .display_point_to_offset_with_bias(
                                DisplayPoint::new(DisplayRow::new(target_row), goal),
                                fold_bias,
                            )
                            .map_err(|error| TextError::InvariantViolation {
                                location: "Editor::move_selections",
                                detail: error.to_string(),
                            })?
                    }
                    Motion::DocumentEdge => match direction {
                        MovementDirection::Previous => MultiBufferOffset::ZERO,
                        MovementDirection::Next => {
                            self.display_snapshot(cx).buffer_snapshot().len_bytes()
                        }
                    },
                };
                // 垂直移动持久保留本次使用的目标列（即使被行尾钳制）；其余移动清除 goal。
                Ok((if extend {
                    selection.with_head(new_head)
                } else {
                    Selection::caret(new_head)
                })
                .with_goal(vertical_goal.map(DisplayColumn::get)))
            })
            .collect::<TextResult<Vec<_>>>()
            .map(|selections| SelectionSet::new_with_primary(selections, primary_index));
        match outcome {
            Ok(selections) => {
                self.composition = None;
                // 普通光标移动结束结构化选择扩展链，避免下次收缩跳回移动前选区。
                self.structured_selection_history.clear();
                self.selections = selections.anchored(self.display_snapshot(cx).buffer_snapshot());
                if matches!(motion, Motion::PageStep(_)) {
                    let display_snapshot = self.display_snapshot(cx).clone();
                    self.scroll_manager
                        .scroll_page(direction == MovementDirection::Next, &display_snapshot);
                }
                self.request_autoscroll(cx);
                self.input_layout = None;
                self.blink_manager.update(cx, |blink, cx| {
                    blink.pause_blinking(cx);
                });
                cx.notify();
            }
            Err(error) => cx.emit(EditorEvent::Error(format!("选区移动失败：{error:#}"))),
        }
    }

    fn request_autoscroll(&mut self, cx: &App) {
        let head = self.resolved_selections(cx).primary().head();
        let anchor = self
            .display_snapshot(cx)
            .buffer_snapshot()
            .anchor_at(head, Affinity::After);
        self.scroll_manager.request_autoscroll(anchor);
    }

    /// 导航跳转定位：把光标行固定在视口顶部下方指定行数。
    pub(super) fn request_scroll_to_top(&mut self, offset_rows: usize, cx: &App) {
        let head = self.resolved_selections(cx).primary().head();
        let anchor = self
            .display_snapshot(cx)
            .buffer_snapshot()
            .anchor_at(head, Affinity::After);
        self.scroll_manager
            .request_scroll_to_top(anchor, offset_rows);
    }

    /// 显示投影推进入口。
    ///
    /// DisplayMap 是显示投影的唯一推进者；Editor 只驱动它同步，不保存第二份显示快照。
    /// 模型事件、编辑提交与结构重建都只经此入口；
    /// 选区以源锚点保存，解析时直接读 DisplayMap 持有的快照，不需要逐状态重映射。
    fn advance_snapshots(&mut self, cx: &mut Context<Self>) {
        // 同步组合文本并按当前语言设置刷新 tab 宽度；快照整体由 DisplayMap 持有。
        self.display_map.update(cx, |map, cx| {
            let snapshot = map.snapshot(cx);
            let tab_width = snapshot.buffer_snapshot().language_settings().tab.tab_width;
            map.set_tab_width(tab_width, cx);
        });
        self.research_after_edit(cx);
        // 搜索命中是显示装饰输入：
        // 把 Editor 拥有的匹配锚点解析结果交给显示链投影，输入未变化时 DisplayMap 快速返回。
        let search = self
            .search
            .as_ref()
            .and_then(EditorSearch::decoration_input);
        self.display_map
            .update(cx, |map, cx| map.set_search_decorations(search, cx));
        self.scrollbar_marker_state.invalidate();
        let snapshot = self.display_snapshot(cx);
        // 外部 reload / 基线替换后，长期锚点必须由调用方显式重锚；
        // 普通解析遇到旧代际锚点会显式失败，不能让它自己猜测坐标。
        self.reattach_persistent_anchors(snapshot.buffer_snapshot());
        self.scroll_manager.refresh(&snapshot);
    }

    /// 外部 reload / 基线替换后，把 Editor 的长期锚点显式重锚到当前快照。
    ///
    /// 只处理当前代际之外的锚点；已匹配当前代际的锚点原样保留。
    /// 普通解析不承担跨代际恢复，本入口是调用方对解析失败的显式“重锚”选择。
    fn reattach_persistent_anchors(&mut self, snapshot: &MultiBufferSnapshot) {
        self.selections = self.selections.reattach(snapshot);
        self.structured_selection_history = self
            .structured_selection_history
            .iter()
            .map(|set| set.reattach(snapshot))
            .collect();
        self.pending_selection = self.pending_selection.take().map(|mut pending| {
            pending.anchor = snapshot
                .reattach_anchor(&pending.anchor)
                .unwrap_or(pending.anchor);
            pending.mode = pending.mode.reattach(snapshot);
            pending
        });
        self.scroll_manager.reattach_anchors(snapshot);
    }

    pub(super) fn handle_toggle_fold(
        &mut self,
        _: &ToggleFold,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_fold_at_cursor(cx);
    }

    pub(super) fn handle_unfold_all(
        &mut self,
        _: &UnfoldAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.unfold_all_ranges(cx);
    }

    /// 展开全部折叠。
    fn unfold_all_ranges(&mut self, cx: &mut Context<Self>) {
        let line_count = self.display_snapshot(cx).buffer_snapshot().line_count();
        if let Ok(line_range) = LineRange::new(Line::ZERO, Line::new(line_count))
            && let Err(error) = self
                .display_map
                .update(cx, |map, cx| map.unfold_lines(line_range, cx))
        {
            cx.emit(EditorEvent::Error(format!("展开折叠失败：{error:#}")));
        }
        self.advance_snapshots(cx);
        cx.notify();
    }

    pub(super) fn handle_move_left(
        &mut self,
        _: &MoveLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(
            MovementDirection::Previous,
            MovementUnit::Grapheme,
            false,
            cx,
        );
    }

    pub(super) fn handle_move_right(
        &mut self,
        _: &MoveRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, MovementUnit::Grapheme, false, cx);
    }

    pub(super) fn handle_move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.propagate_if_single_line(cx) {
            return;
        }
        self.move_selections(MovementDirection::Previous, Motion::LineStep, false, cx);
    }

    pub(super) fn handle_move_down(
        &mut self,
        _: &MoveDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.propagate_if_single_line(cx) {
            return;
        }
        self.move_selections(MovementDirection::Next, Motion::LineStep, false, cx);
    }

    pub(super) fn handle_move_to_previous_word(
        &mut self,
        _: &MoveToPreviousWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Previous, MovementUnit::Word, false, cx);
    }

    pub(super) fn handle_move_to_next_word(
        &mut self,
        _: &MoveToNextWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, MovementUnit::Word, false, cx);
    }

    pub(super) fn handle_move_to_beginning_of_line(
        &mut self,
        _: &MoveToBeginningOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(
            MovementDirection::Previous,
            MovementUnit::LineEdge,
            false,
            cx,
        );
    }

    pub(super) fn handle_move_to_end_of_line(
        &mut self,
        _: &MoveToEndOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, MovementUnit::LineEdge, false, cx);
    }

    pub(super) fn handle_move_to_beginning(
        &mut self,
        _: &MoveToBeginning,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.propagate_if_single_line(cx) {
            return;
        }
        self.move_selections(MovementDirection::Previous, Motion::DocumentEdge, false, cx);
    }

    pub(super) fn handle_move_to_end(
        &mut self,
        _: &MoveToEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.propagate_if_single_line(cx) {
            return;
        }
        self.move_selections(MovementDirection::Next, Motion::DocumentEdge, false, cx);
    }

    pub(super) fn handle_move_page_up(
        &mut self,
        _: &MovePageUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.propagate_if_single_line(cx) {
            return;
        }
        let Some(row_count) = self.scroll_manager.page_row_count() else {
            return;
        };
        self.move_selections(
            MovementDirection::Previous,
            Motion::PageStep(row_count),
            false,
            cx,
        );
    }

    pub(super) fn handle_move_page_down(
        &mut self,
        _: &MovePageDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.propagate_if_single_line(cx) {
            return;
        }
        let Some(row_count) = self.scroll_manager.page_row_count() else {
            return;
        };
        self.move_selections(
            MovementDirection::Next,
            Motion::PageStep(row_count),
            false,
            cx,
        );
    }

    pub(super) fn handle_select_left(
        &mut self,
        _: &SelectLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(
            MovementDirection::Previous,
            MovementUnit::Grapheme,
            true,
            cx,
        );
    }

    pub(super) fn handle_select_right(
        &mut self,
        _: &SelectRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, MovementUnit::Grapheme, true, cx);
    }

    pub(super) fn handle_select_up(
        &mut self,
        _: &SelectUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Previous, Motion::LineStep, true, cx);
    }

    pub(super) fn handle_select_down(
        &mut self,
        _: &SelectDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, Motion::LineStep, true, cx);
    }

    pub(super) fn handle_select_to_previous_word(
        &mut self,
        _: &SelectToPreviousWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Previous, MovementUnit::Word, true, cx);
    }

    pub(super) fn handle_select_to_next_word(
        &mut self,
        _: &SelectToNextWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, MovementUnit::Word, true, cx);
    }

    pub(super) fn handle_select_to_beginning_of_line(
        &mut self,
        _: &SelectToBeginningOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(
            MovementDirection::Previous,
            MovementUnit::LineEdge,
            true,
            cx,
        );
    }

    pub(super) fn handle_select_to_end_of_line(
        &mut self,
        _: &SelectToEndOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, MovementUnit::LineEdge, true, cx);
    }

    pub(super) fn handle_select_to_beginning(
        &mut self,
        _: &SelectToBeginning,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Previous, Motion::DocumentEdge, true, cx);
    }

    pub(super) fn handle_select_to_end(
        &mut self,
        _: &SelectToEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, Motion::DocumentEdge, true, cx);
    }

    pub(super) fn handle_select_page_up(
        &mut self,
        _: &SelectPageUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row_count) = self.scroll_manager.page_row_count() else {
            return;
        };
        self.move_selections(
            MovementDirection::Previous,
            Motion::PageStep(row_count),
            true,
            cx,
        );
    }

    pub(super) fn handle_select_page_down(
        &mut self,
        _: &SelectPageDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row_count) = self.scroll_manager.page_row_count() else {
            return;
        };
        self.move_selections(
            MovementDirection::Next,
            Motion::PageStep(row_count),
            true,
            cx,
        );
    }

    pub(super) fn handle_select_all(
        &mut self,
        _: &SelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let end = self.display_snapshot(cx).buffer_snapshot().len_bytes();
        self.change_selections(
            SelectionSet::new(vec![Selection::new(MultiBufferOffset::ZERO, end)]),
            cx,
        );
    }

    pub(super) fn handle_select_larger_syntax_node(
        &mut self,
        _: &SelectLargerSyntaxNode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.resolved_selections(cx);
        let expanded = current
            .as_slice()
            .iter()
            .map(|selection| {
                let range = selection.start().get()..selection.end().get();
                self.display_snapshot(cx)
                    .ancestor_range(range)
                    .map(|range| {
                        Selection::new(
                            MultiBufferOffset::new(range.start),
                            MultiBufferOffset::new(range.end),
                        )
                    })
                    .unwrap_or(*selection)
            })
            .collect();
        let expanded = SelectionSet::new_with_primary(expanded, current.primary_index());
        if expanded == current {
            return;
        }
        self.structured_selection_history
            .push(self.selections.clone());
        self.apply_selection_change(expanded, cx);
    }

    pub(super) fn handle_select_smaller_syntax_node(
        &mut self,
        _: &SelectSmallerSyntaxNode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(previous) = self.structured_selection_history.pop() else {
            return;
        };
        // 结构化选择恢复是显式入口：直接恢复锚点集合，不经过常规选区清空路径。
        self.composition = None;
        self.selections = previous;
        self.request_autoscroll(cx);
        self.input_layout = None;
        cx.notify();
    }

    /// 键位上下文：Editor 标识 + mode 标签。
    /// keymap 据此按模式选择绑定（如 `Editor && mode == full`），输入框类编辑器（single_line/auto_height）不占用 enter/tab 等键位。
    fn key_context(&self) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        context.add("Editor");
        context.set(
            "mode",
            match self.mode {
                EditorMode::SingleLine => "single_line",
                EditorMode::AutoHeight { .. } => "auto_height",
                EditorMode::Full => "full",
            },
        );
        context
    }
}

fn layout_line_width(
    display_snapshot: &DisplaySnapshot,
    row: DisplayRow,
    font: &gpui::Font,
    font_size: Pixels,
    window: &mut Window,
) -> Pixels {
    let mut rows = display_snapshot.rows(row, 1);
    let Some(row) = rows.next() else {
        return Pixels::ZERO;
    };
    if row.block().is_some() {
        return Pixels::ZERO;
    }
    let WrapRowKind::Text {
        byte_range,
        projected_line,
        ..
    } = row.kind();
    let Some(row_text) = display_snapshot.row_text(*projected_line) else {
        return Pixels::ZERO;
    };
    // 行投影保留行终止符以维持 byte 坐标；文本测量 API 只接受单行内容。
    let text = &row_text.as_ref()[byte_range.clone()];
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text_style = window.text_style();
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: text_style.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text.to_owned().into(), font_size, &[run], None)
        .width
}

impl EventEmitter<EditorEvent> for Editor {}

impl gpui::Focusable for Editor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 首次渲染时注册焦点事件（构造函数中没有 Window）
        if !self.blink_manager_initialized {
            cx.on_focus(&self.focus, window, |editor, window, cx| {
                editor.sync_cursor_blinking(window, cx);
            })
            .detach();
            cx.on_blur(&self.focus, window, |editor, window, cx| {
                editor.sync_cursor_blinking(window, cx);
            })
            .detach();
            cx.observe_window_activation(window, |editor, window, cx| {
                editor.sync_cursor_blinking(window, cx);
            })
            .detach();
            self.blink_manager_initialized = true;
        }

        // 弥补焦点或窗口激活先于首次 render 到达的时序缺口。
        self.sync_cursor_blinking(window, cx);

        // 普通 SingleLine / AutoHeight 用于搜索框等 UI 场景，应使用 UI 排版；
        // 嵌入代码编辑器的单行输入（如重命名）显式跟随内容排版。
        let type_scale = typography_for_window(window, cx);
        let (font, text_size, line_height) =
            if self.mode == EditorMode::Full || self.content_typography {
                (
                    typography::content_font(),
                    type_scale.content_size(),
                    type_scale.content_line(),
                )
            } else {
                (
                    typography::ui_font(),
                    type_scale.ui_size(),
                    type_scale.ui_line(),
                )
            };
        let visible_lines = match self.mode {
            EditorMode::SingleLine => Some(1),
            EditorMode::AutoHeight {
                min_lines,
                max_lines,
            } => {
                let line_count = self.display_snapshot(cx).line_count().max(min_lines);
                Some(max_lines.map_or(line_count, |maximum| line_count.min(maximum)))
            }
            EditorMode::Full => None,
        };

        let local_rename = self.local_rename_overlay();
        let colors = *color::current(cx);
        EditorElement::register_actions(
            div()
                .track_focus(&self.focus)
                .key_context(self.key_context())
                .tab_index(0)
                .cursor(CursorStyle::IBeam)
                .w_full()
                .when_some(visible_lines, |element, lines| {
                    element.h(line_height * lines)
                })
                .when(visible_lines.is_none(), |element| element.flex_1().h_full())
                .overflow_hidden()
                .font(font)
                .text_size(text_size)
                .line_height(line_height)
                .text_color(colors.text)
                .relative(),
            cx,
        )
        .child(EditorElement::new(cx.entity()))
        .when_some(
            local_rename,
            |element, (input, position, width, line_height)| {
                element.child(
                    div()
                        .absolute()
                        .left(position.x)
                        .top(position.y)
                        .w(width)
                        .h(line_height)
                        .p(gpui::px(0.))
                        .rounded_sm()
                        .child(input),
                )
            },
        )
        .into_any_element()
    }
}

#[cfg(test)]
#[path = "test/selection_edit_tests.rs"]
mod selection_edit_tests;

#[cfg(test)]
#[path = "test/auto_pair_tests.rs"]
mod auto_pair_tests;

#[cfg(test)]
#[path = "test/search_tests.rs"]
mod search_tests;

mod editing;
mod input;

use input::AutocloseRegion;
/// 输入法组合会话与展示快照（element 渲染 marked ranges 用）。
pub(super) use input::EditorComposition;
pub(super) use presentation::EditorPresentation;

#[cfg(test)]
#[path = "test/common.rs"]
mod common;

#[cfg(test)]
#[path = "test/actions_tests.rs"]
mod actions_tests;

#[cfg(test)]
#[path = "test/ime_tests.rs"]
mod ime_tests;

#[cfg(test)]
#[path = "test/display_tests.rs"]
mod display_tests;

#[cfg(test)]
#[path = "test/scroll_tests.rs"]
mod scroll_tests;

#[cfg(test)]
#[path = "test/mouse_selection_tests.rs"]
mod mouse_selection_tests;

#[cfg(test)]
#[path = "test/cursor_activation_tests.rs"]
mod cursor_activation_tests;

#[cfg(test)]
#[path = "test/conflict_hunk_tests.rs"]
mod conflict_hunk_tests;

/// 编辑事务元数据（供编辑命令与输入共用）。
pub(super) fn edit_metadata(description: &'static str) -> TransactionMetadata {
    TransactionMetadata::new(TransactionSource::Programmatic).with_description(description)
}
