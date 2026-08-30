//! Editor View 的跨帧状态与交互。

/// 导航跳转（打开文件/行列定位）时目标行距视口顶部的固定行数，留出上下文。
pub(super) const NAVIGATION_TOP_OFFSET: usize = 4;

use std::cell::Cell;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, Entity, EventEmitter, FocusHandle, IntoElement,
    Pixels, Point, Render, Styled, Window, div, point, prelude::*,
};
use zcv_actions::{
    Backspace, Copy, Cut, Delete, DeleteToBeginningOfLine, DeleteToEndOfLine, DeleteToNextWordEnd,
    DeleteToPreviousWordStart, ExpandSelection, Indent, MoveDown, MoveLeft, MoveLineDown,
    MoveLineUp, MovePageDown, MovePageUp, MoveRight, MoveToBeginning, MoveToBeginningOfLine,
    MoveToEnd, MoveToEndOfLine, MoveToNextWord, MoveToPreviousWord, MoveUp, Newline, OpenExcerpts,
    Outdent, Paste, Redo, SelectAll, SelectDown, SelectLeft, SelectPageDown, SelectPageUp,
    SelectRight, SelectToBeginning, SelectToBeginningOfLine, SelectToEnd, SelectToEndOfLine,
    SelectToNextWord, SelectToPreviousWord, SelectUp, ToggleFold, Undo, UnfoldAll,
};
use zcv_git::{DiffHunk, DiffHunkKind};
use zcv_language::{AutoClosePair, BracketPair, FoldRange, LanguageBuffer};
use zcv_multi_buffer::{
    ExcerptDiffKind, ExcerptLocation, ExcerptSnapshot, MultiBuffer, MultiBufferAnchor,
    MultiBufferEvent, MultiBufferSubscription,
};
use zcv_settings::{SettingsStore, SoftWrapMode};
use zcv_text::{
    Buffer, BufferConfig, BufferVersion, ByteOffset, DeltaEvent, Edit, Line, LineRange,
    LogicalColumn, MovementDirection, MovementUnit, Position, PositionMap, Snapshot, TextRange,
    TextResult, TransactionId, TransactionMergePolicy, TransactionMetadata, TransactionSource,
};
use zcv_theme::{color, typography};

use super::blink_manager::BlinkManager;
use super::display_map::{DisplayColumn, DisplayMap, DisplayPoint, DisplayRow, DisplaySnapshot};
use super::element::{AUTOSCROLL_INTERVAL, EditorElement, EditorInputLayout};
use super::scroll::{ScrollManager, ScrollbarThumbState};
use super::selection::{
    EditOutcome, EditorSelections, Selection, SelectionHistory, SelectionSet, replace_selections,
};

mod diff;
mod search;

pub(crate) use diff::{HunkRendering, diff_kind_for_row, hunk_rendering};
pub(crate) use search::EditorSearch;

/// Editor 自身的领域事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    /// 编辑器关联的文件路径发生变化。
    PathChanged,
    /// 文档内容被编辑。
    Edited,
    /// 文档是否包含未保存修改发生变化。
    DirtyChanged,
    /// 复合文档请求宿主打开底层文件。
    OpenExcerptsRequested {
        locations: Vec<ExcerptLocation>,
        split: bool,
    },
    /// 删除/修改块的展开折叠状态变化（宿主按展开状态重建组合文档内容）。
    DiffHunksExpandedChanged,
}

/// Editor 负责把控件定位到 hunk 右上角，具体按钮与操作由宿主视图提供。
pub trait DiffHunkDelegate {
    fn render_hunk_controls(
        &self,
        row: usize,
        hunk: &DiffHunk,
        line_height: Pixels,
        editor: &Entity<Editor>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement;
}

/// 绑定到底层文件位置的编辑器视口锚点。
///
/// 与显示行号不同，该锚点可在组合文档 excerpts 增删、重排后重新解析。
#[derive(Clone, Debug)]
pub struct EditorScrollAnchor {
    buffer_anchor: MultiBufferAnchor,
    preceding_virtual_rows: usize,
    offset: Point<Pixels>,
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
    Word(Range<ByteOffset>),
    Line(Range<ByteOffset>),
    All,
}

/// 拖拽中的选区状态：固定锚点 + 点击时的粒度。
#[derive(Debug, Clone)]
struct PendingSelection {
    /// 按下点字节偏移，字符粒度拖拽的固定端。
    anchor: ByteOffset,
    /// 点击时的粒度与锚定范围。
    mode: MouseSelectMode,
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

/// 软换行模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SoftWrap {
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

pub struct Editor {
    multi_buffer: Entity<MultiBuffer>,
    multi_buffer_subscription: MultiBufferSubscription,
    last_dirty: bool,
    display_map: DisplayMap,
    mode: EditorMode,
    /// 空 buffer 时显示的提示文本（如提交信息编辑器的"输入提交信息…"）。
    /// 独立 DisplayMap 承载（placeholder 走真实渲染管线，折行/行高一致）。
    placeholder_display_map: Option<DisplayMap>,
    selections: EditorSelections,
    selection_history: SelectionHistory,
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
    /// 拖拽选择自动滚动的限频时间戳（跨帧持久；事件频率可远超帧率，滚动频率需封顶）。
    pub(crate) last_drag_autoscroll: Cell<Instant>,
    /// 文件内搜索状态（搜索条执行过一次搜索后存在，编辑后自动重搜）。
    search: Option<EditorSearch>,
    /// 语言层提供的可折叠范围（crease 显示与折叠命令的数据源；
    /// 在 buffer 编辑或语法快照更新时刷新）。
    /// 折叠范围（共享 LanguageBuffer 缓存：Reparsed 后整体替换，多个 Editor 复用同一份）。
    fold_ranges: Arc<[FoldRange]>,
    /// 匹配括号缓存：键 = (primary head, buffer 版本, 语法版本)。
    /// 光标移动或任一版本推进即重查；
    /// 滚动/纯重绘帧直接命中，不再跑 tree-sitter 查询。
    bracket_pair_cache: Option<(
        ByteOffset,
        BufferVersion,
        BufferVersion,
        Option<BracketPair>,
    )>,
    /// 最近一次鼠标手势的选区粒度；Shift+点击时按此粒度扩展。
    mouse_select_mode: MouseSelectMode,
    /// 正在进行的鼠标选区手势；普通选区变更会终止它。
    pending_selection: Option<PendingSelection>,
    /// 自动补全的闭合符标记（输入闭合符时跳过、退格删除整对的数据源）。
    /// 随每次编辑经 PositionMap 推进；区域版本与当前快照失配（未跟踪的外部编辑）时整体失效。
    autoclose_regions: Vec<AutocloseRegion>,
}

impl Editor {
    pub fn single_line(cx: &mut Context<Self>) -> Self {
        let buffer = Buffer::scratch(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");
        let buffer = cx.new(|_| buffer);
        let language_buffer = cx.new(|cx| LanguageBuffer::new(buffer, None, cx));
        Self::from_language_buffer(language_buffer, EditorMode::SingleLine, cx)
    }

    pub fn auto_height(min_lines: usize, max_lines: Option<usize>, cx: &mut Context<Self>) -> Self {
        let buffer = Buffer::scratch(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");
        let buffer = cx.new(|_| buffer);
        let language_buffer = cx.new(|cx| LanguageBuffer::new(buffer, None, cx));
        Self::from_language_buffer(
            language_buffer,
            EditorMode::AutoHeight {
                min_lines,
                max_lines,
            },
            cx,
        )
    }

    #[cfg(test)]
    fn for_language_buffer(
        language_buffer: Entity<LanguageBuffer>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::from_language_buffer(language_buffer, EditorMode::Full, cx)
    }

    pub fn for_multi_buffer(multi_buffer: Entity<MultiBuffer>, cx: &mut Context<Self>) -> Self {
        Self::new(multi_buffer, EditorMode::Full, cx)
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 本地 caret 是否应当绘制。
    ///
    /// 只读 Editor 仍保留稳定可见的 caret，使 MultiBuffer 与普通 Editor 共用同一套定位和选区反馈；
    /// 可编辑 Editor 才由 BlinkManager 控制闪烁。
    pub(crate) fn show_cursor(&self, window: &Window, cx: &App) -> bool {
        window.is_window_active()
            && self.focus.is_focused(window)
            && (self.is_read_only(cx) || self.blink_manager.read(cx).visible())
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

    pub(crate) fn text_buffer(&self, cx: &App) -> Entity<Buffer> {
        self.multi_buffer.read(cx).text_buffer(cx)
    }

    pub fn is_read_only(&self, cx: &App) -> bool {
        self.multi_buffer.read(cx).is_read_only()
    }

    pub fn excerpt_location(&self, cx: &App) -> Option<ExcerptLocation> {
        let range = self.resolved_selections().primary().range();
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
                source_range: TextRange::new(start, start).expect("同点源范围必须有效"),
            }],
            split,
        });
    }

    pub(crate) fn is_buffer_folded(&self, path: &std::path::Path) -> bool {
        self.display_map.is_buffer_folded(path)
    }

    /// 折叠/展开 MultiBuffer 中一个文件的全部 excerpts。
    /// 这是 BlockMap 变换，不修改组合文本，也不借用语法折叠范围。
    pub(crate) fn toggle_buffer_fold(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let folded = !self.display_map.is_buffer_folded(&path);
        self.display_map.set_buffer_folded(path, folded);
        self.request_autoscroll();
        self.input_layout = None;
        cx.notify();
    }

    pub(super) fn handle_open_excerpts(
        &mut self,
        _: &OpenExcerpts,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.multi_buffer.read(cx).as_singleton(cx).is_some() {
            cx.propagate();
            return;
        }
        let locations = self
            .resolved_selections()
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

    /// 覆盖换行模式（UI 场景强制使用，不随全局设置变化）；`None` 清除覆盖恢复设置值。
    ///
    /// 实际换行在下一帧 prepaint 计算 wrap 宽度时生效。
    pub fn set_soft_wrap_mode(&mut self, soft_wrap: Option<SoftWrap>, cx: &mut Context<Self>) {
        if self.soft_wrap_override == soft_wrap {
            return;
        }
        self.soft_wrap_override = soft_wrap;
        cx.notify();
    }

    /// 生效的换行模式：覆盖优先，否则跟随全局设置。
    pub(crate) fn soft_wrap(&self) -> SoftWrap {
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
    ) {
        let text_system = cx.text_system();
        self.display_map
            .set_wrap_width(wrap_width, font, font_size, text_system);
    }

    pub fn file_path(&self, cx: &App) -> Option<PathBuf> {
        self.multi_buffer.read(cx).file_path(cx)
    }

    pub fn language_name(&self, cx: &App) -> Option<&'static str> {
        // 组合文档按光标所在 excerpt 的源语言显示。
        let offset = self.resolved_selections().primary().head();
        self.multi_buffer.read(cx).language_at(offset, cx)
    }

    pub fn set_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.multi_buffer
            .update(cx, |buffer, cx| buffer.set_file_path(path, cx));
        cx.emit(EditorEvent::PathChanged);
    }

    /// 注入滚动轴 marker 的行级 diff hunks（git 状态刷新后由 bin 层调用）。
    ///
    /// `None` 是加载态（新 diff 尚未算完）：保留现有 hunks 与用户展开状态，不再被中间空列表清空；展开状态按工作区文本锚点跨刷新迁移。
    /// 状态与投影归属 MultiBuffer，本方法只转发并同步视图层状态。
    /// 返回 `true` 表示组合文档被重建（光标已落回开头）。
    pub fn set_diff_hunks(&mut self, hunks: Option<Vec<DiffHunk>>, cx: &mut Context<Self>) -> bool {
        let rebuilt = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.set_diff_hunks(hunks, cx));
        self.after_diff_projection_rebuild(rebuilt, cx);
        rebuilt
    }

    /// 注入旧侧已经属于 MultiBuffer 投影文本的 diff hunks（多文件投影模式）。
    pub fn set_materialized_diff_hunks(
        &mut self,
        hunks: Vec<zcv_multi_buffer::MaterializedDiffHunk>,
        cx: &mut Context<Self>,
    ) {
        self.multi_buffer.update(cx, |buffer, cx| {
            buffer.set_materialized_diff_hunks(hunks, cx)
        });
        cx.notify();
    }

    pub fn set_diff_hunk_delegate(
        &mut self,
        delegate: Option<Arc<dyn DiffHunkDelegate>>,
        cx: &mut Context<Self>,
    ) {
        self.diff_hunk_delegate = delegate;
        cx.notify();
    }

    pub(crate) fn diff_hunk_delegate(&self) -> Option<Arc<dyn DiffHunkDelegate>> {
        self.diff_hunk_delegate.clone()
    }

    /// 设置新 hunk 的初始展开策略；用户之后的显式展开/折叠不受投影刷新覆盖。
    pub fn set_diff_hunks_expanded_by_default(&mut self, expanded: bool, cx: &mut Context<Self>) {
        let rebuilt = self.multi_buffer.update(cx, |buffer, cx| {
            buffer.set_diff_hunks_expanded_by_default(expanded, cx)
        });
        self.after_diff_projection_rebuild(rebuilt, cx);
    }

    /// hunk 的当前展开状态；尚未进入投影的新 hunk 直接采用默认策略。
    pub fn is_diff_hunk_expanded(
        &self,
        kind: DiffHunkKind,
        old_range: &Range<usize>,
        cx: &App,
    ) -> bool {
        self.multi_buffer
            .read(cx)
            .is_diff_hunk_expanded(kind, old_range)
    }

    /// base 版本变化后由宿主调用：重置展开状态（materialized 模式旧侧坐标空间失效时）。
    pub fn reset_diff_hunk_expansion_state(&mut self, cx: &mut Context<Self>) {
        let rebuilt = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.reset_diff_hunk_expansion_state(cx));
        self.after_diff_projection_rebuild(rebuilt, cx);
    }

    /// 已展开的删除 hunk（按旧侧行范围标识；渲染背景色用，宿主重建片段按此判断）。
    pub fn expanded_deleted_hunks<'a>(&self, cx: &'a App) -> &'a [Range<usize>] {
        self.multi_buffer.read(cx).expanded_deleted_hunks()
    }

    /// 已展开的修改 hunk（按旧侧行范围标识；渲染背景色用，宿主重建片段按此判断）。
    pub fn expanded_modified_hunks<'a>(&self, cx: &'a App) -> &'a [Range<usize>] {
        self.multi_buffer.read(cx).expanded_modified_hunks()
    }

    /// 注入 HEAD 全文（删除块展开的数据源）；到达后重建删除块。
    pub fn set_deleted_hunk_text(&mut self, text: Option<Arc<str>>, cx: &mut Context<Self>) {
        let rebuilt = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.set_diff_head_text(text, cx));
        self.after_diff_projection_rebuild(rebuilt, cx);
    }

    /// diff 投影重建后同步视图层状态：组合文档文本版本变化时同步 DisplayMap 并把光标落回开头。
    fn after_diff_projection_rebuild(&mut self, rebuilt: bool, cx: &mut Context<Self>) {
        if rebuilt {
            // 组合文本整体替换（diff 投影重建）：同步 DisplayMap 并把光标落回开头；
            // 延迟到达的 TextChanged 事件会再次 sync，按版本比对跳过重复映射。
            self.sync_display_map(cx);
            let version = self.text_buffer(cx).read(cx).snapshot().version();
            self.selections =
                EditorSelections::from_selection_set(version, &SelectionSet::default());
        }
        cx.notify();
    }

    /// 语言层可折叠范围（crease 渲染与折叠命令共用）。
    pub(crate) fn fold_ranges(&self) -> &[FoldRange] {
        &self.fold_ranges
    }

    /// 展开/折叠删除块（按 hunk 的 old_range 标识）。
    pub fn toggle_deleted_hunk(&mut self, old_range: Range<usize>, cx: &mut Context<Self>) {
        let rebuilt = self.multi_buffer.update(cx, |buffer, cx| {
            buffer.toggle_diff_hunk(DiffHunkKind::Deleted, old_range, cx)
        });
        self.after_diff_projection_rebuild(rebuilt, cx);
    }

    /// 展开/折叠修改块：展开显示修改前的 HEAD 旧行（base 旧行插在修改行上方）。
    pub fn toggle_modified_hunk(&mut self, old_range: Range<usize>, cx: &mut Context<Self>) {
        let rebuilt = self.multi_buffer.update(cx, |buffer, cx| {
            buffer.toggle_diff_hunk(DiffHunkKind::Modified, old_range, cx)
        });
        self.after_diff_projection_rebuild(rebuilt, cx);
    }

    /// 折叠/展开指定逻辑行（crease 点击与 ToggleFold 命令的共享实现）。
    ///
    /// 该行是折叠入口行则展开覆盖它的折叠；否则若该行是可折叠范围起点则折叠整个范围。
    pub(crate) fn toggle_fold_at_line(&mut self, line: Line, cx: &mut Context<Self>) {
        let display_snapshot = self.display_map.snapshot();
        if display_snapshot.fold_anchor_lines().contains(&line) {
            let line_range =
                LineRange::new(line, Line::new(line.get() + 1)).expect("光标行 +1 应合法");
            if let Err(error) = self.display_map.unfold_lines(line_range) {
                eprintln!("展开折叠失败：{error}");
            }
        } else {
            let snapshot = self.render_snapshot();
            let range = self.fold_ranges.iter().find(|range| {
                snapshot
                    .byte_to_line(ByteOffset::new(range.range.start))
                    .is_ok_and(|start| start == line)
            });
            if let Some(range) = range
                && let Ok(start) = snapshot.byte_to_line(ByteOffset::new(range.range.start))
                && start == line
            {
                // 折叠范围是字节级的（终点在闭合括号前）：直接按字节范围折叠。
                if let Err(error) = self.display_map.fold_range(
                    TextRange::new(
                        ByteOffset::new(range.range.start),
                        ByteOffset::new(range.range.end),
                    )
                    .expect("折叠范围应合法"),
                ) {
                    eprintln!("折叠失败：{error}");
                }
            }
        }
        cx.notify();
    }

    /// 按当前光标所在显示行切换折叠。
    ///
    /// 已折叠时，入口文本、占位符与闭合尾段属于同一显示行，从其中任意位置触发都展开该行的折叠；
    /// 未折叠时，折叠包含光标逻辑行的最内层范围。
    fn toggle_fold_at_cursor(&mut self, cx: &mut Context<Self>) {
        let head = self.resolved_selections().primary().head();
        let display_snapshot = self.display_map.snapshot();
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
                if let Err(error) = self.display_map.unfold_lines(line_range) {
                    eprintln!("展开折叠失败：{error}");
                }
            }
            cx.notify();
            return;
        }

        let snapshot = self.render_snapshot();
        let Ok(head_line) = snapshot.byte_to_line(head) else {
            return;
        };
        let range = self
            .fold_ranges
            .iter()
            .filter_map(|range| {
                let start = snapshot
                    .byte_to_line(ByteOffset::new(range.range.start))
                    .ok()?;
                let end = snapshot
                    .byte_to_line(ByteOffset::new(range.range.end))
                    .ok()?;
                (start <= head_line && head_line <= end).then_some((range, start, end))
            })
            .min_by_key(|(_, start, end)| (head_line.get() - start.get(), end.get() - start.get()))
            .map(|(range, _, _)| range.range.clone());

        if let Some(range) = range
            && let Err(error) = self.display_map.fold_range(
                TextRange::new(ByteOffset::new(range.start), ByteOffset::new(range.end))
                    .expect("折叠范围应合法"),
            )
        {
            eprintln!("折叠失败：{error}");
        }
        cx.notify();
    }

    /// 与当前 buffer 版本匹配的 diff hunks；未注入或注入后发生编辑时返回空。
    pub(crate) fn diff_hunks<'a>(&self, cx: &'a App) -> &'a [DiffHunk] {
        self.multi_buffer.read(cx).diff_hunks(cx)
    }

    pub(crate) fn diff_hunk_old_ranges<'a>(&self, cx: &'a App) -> &'a [Option<Range<usize>>] {
        self.multi_buffer.read(cx).diff_hunk_old_ranges(cx)
    }

    pub fn text(&self, cx: &App) -> String {
        let snapshot = self.text_buffer(cx).read(cx).snapshot();
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .expect("完整 Editor Snapshot 范围必须可读取")
            .as_str()
            .to_owned()
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        self.multi_buffer.read(cx).is_dirty(cx)
    }

    /// 设置空 buffer 时显示的提示文本。
    ///
    /// 文本放进独立 DisplayMap：渲染层在空 buffer 时把它的快照接入行管线，
    /// 折行/行高/滚动与真实文本一致；空文本清除 placeholder。
    pub fn set_placeholder_text(&mut self, text: impl Into<String>, _cx: &mut Context<Self>) {
        let text = text.into();
        self.placeholder_display_map = if text.is_empty() {
            None
        } else {
            let buffer = Buffer::scratch(text, BufferConfig::default())
                .expect("placeholder Buffer 应能创建");
            Some(DisplayMap::new(buffer.snapshot()))
        };
    }

    /// 空 buffer 且有 placeholder 时返回其快照（渲染层行数据源替换用）。
    pub(super) fn placeholder_snapshot_if_empty(&self, cx: &App) -> Option<DisplaySnapshot> {
        if !self.text(cx).is_empty() {
            return None;
        }
        self.placeholder_display_map
            .as_ref()
            .map(|map| map.snapshot())
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.is_read_only(cx) {
            return;
        }
        self.composition = None;
        let before_selections = self.resolved_selections();
        let targets = SelectionSet::new(vec![Selection::new(
            ByteOffset::ZERO,
            self.text_buffer(cx).read(cx).len_bytes(),
        )]);
        let text = if self.mode == EditorMode::SingleLine {
            text.replace(['\r', '\n'], "")
        } else {
            text.to_owned()
        };
        let _ = self.change_with_after(before_selections, cx, |buffer| {
            replace_selections(buffer, &targets, &text, edit_metadata("设置文本"))
        });
    }

    /// 将单个选择区设置为给定的 UTF-8 字节范围。
    pub fn select_byte_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let end = self.text_buffer(cx).read(cx).len_bytes();
        assert!(range.start <= range.end && ByteOffset::new(range.end) <= end);
        self.change_selections(
            SelectionSet::new(vec![Selection::new(
                ByteOffset::new(range.start),
                ByteOffset::new(range.end),
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
        let snapshot = self.render_snapshot();
        let Ok(offset) =
            snapshot.position_to_byte(Position::new(Line::new(line), LogicalColumn::new(column)))
        else {
            return false;
        };
        self.change_selections(SelectionSet::caret(offset), cx);
        self.request_scroll_to_top(NAVIGATION_TOP_OFFSET);
        true
    }

    /// 选区变更样板：结束组合会话、重锚定选区、请求自动滚动并清空 IME 布局缓存。
    pub(super) fn change_selections(&mut self, selections: SelectionSet, cx: &mut Context<Self>) {
        self.composition = None;
        self.set_selections(selections);
        self.request_autoscroll();
        self.input_layout = None;
        cx.notify();
    }

    pub fn render_snapshot(&self) -> Snapshot {
        self.display_map.buffer_snapshot().clone()
    }

    pub(super) fn display_snapshot(&self) -> DisplaySnapshot {
        self.display_map.snapshot()
    }

    pub(super) fn matching_bracket_pair(&mut self) -> Option<BracketPair> {
        // 任一选区非空时都不显示匹配括号高亮，避免把括号强调误认成选区的一部分；
        // 判断在缓存之前，选区状态变化不会命中陈旧缓存。
        let selections = self.resolved_selections();
        if selections
            .as_slice()
            .iter()
            .any(|selection| !selection.is_caret())
        {
            return None;
        }
        let snapshot = self.display_map.buffer_snapshot();
        let caret = selections.primary().head();
        let buffer_version = snapshot.version();
        let syntax_snapshot = self.display_map.syntax_snapshot();
        let syntax_version = syntax_snapshot.version();
        if let Some((cached_caret, cached_buffer, cached_syntax, cached)) = &self.bracket_pair_cache
            && *cached_caret == caret
            && *cached_buffer == buffer_version
            && *cached_syntax == syntax_version
        {
            return cached.clone();
        }
        let caret_offset = caret.get();
        let start = caret_offset.saturating_sub(1);
        let end = caret_offset
            .saturating_add(1)
            .min(snapshot.len_bytes().get());
        let result = syntax_snapshot
            .bracket_pairs(start..end, snapshot)
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
        self.bracket_pair_cache = Some((caret, buffer_version, syntax_version, result.clone()));
        result
    }

    pub(crate) fn selections(&self) -> SelectionSet {
        self.resolved_selections()
    }

    /// 把 offset 版选区集合重锚定到当前显示快照版本。
    pub(crate) fn set_selections(&mut self, selections: SelectionSet) {
        // 任何普通选区替换都会终止 pending selection，避免旧鼠标锚点在之后复活。
        self.pending_selection = None;
        self.set_pending_selection(selections);
    }

    /// 更新 pending selection 显示出的当前选区。
    /// 只有 begin/update selection 可以调用这个入口。
    fn set_pending_selection(&mut self, selections: SelectionSet) {
        let version = self.display_map.buffer_snapshot().version();
        self.selections = EditorSelections::from_selection_set(version, &selections);
    }

    /// 按当前显示快照把端点锚点解析为 offset 版选区集合。
    fn resolved_selections(&self) -> SelectionSet {
        self.selections.resolve(self.display_map.buffer_snapshot())
    }

    /// 光标位置的 "行:列" 文本，行和列均从 1 开始计数。
    /// 组合文档（多文件编辑器）按光标所在 excerpt 映射回源文件内的真实行列。
    pub fn cursor_text(&self, cx: &App) -> String {
        let head = self.resolved_selections().primary().head();
        let multi_snapshot = self.multi_buffer.read(cx).snapshot(cx);
        let Ok(point) = multi_snapshot.text().byte_to_position(head) else {
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

    pub(super) fn presentation(&self) -> EditorPresentation {
        EditorPresentation::new(
            self.display_map.buffer_snapshot(),
            self.composition.as_ref(),
        )
    }

    pub(super) fn shows_gutter(&self) -> bool {
        self.mode == EditorMode::Full
    }

    pub(super) fn active_lines(&self) -> Vec<Line> {
        touched_lines(
            self.display_map.buffer_snapshot(),
            &self.resolved_selections(),
        )
        .unwrap_or_default()
    }

    pub(super) fn scroll_anchor(&self) -> DisplayPoint {
        self.scroll_manager.anchor()
    }

    pub(super) fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll_manager.offset()
    }

    /// 捕获当前视口顶部对应的底层文件位置。
    pub fn capture_scroll_anchor(&mut self, cx: &App) -> Option<EditorScrollAnchor> {
        self.sync_display_map(cx);
        let display_point = self.scroll_manager.anchor();
        let output_offset = self
            .display_map
            .display_point_to_offset(display_point)
            .ok()?;
        let canonical_point = self
            .display_map
            .offset_to_display_point(output_offset)
            .ok()?;
        let buffer_anchor = self
            .multi_buffer
            .read(cx)
            .anchor_for_offset(output_offset)?;
        Some(EditorScrollAnchor {
            buffer_anchor,
            preceding_virtual_rows: canonical_point
                .row()
                .get()
                .saturating_sub(display_point.row().get()),
            offset: self.scroll_manager.offset(),
        })
    }

    /// 在组合文档结构刷新后恢复视口；目标片段消失时由 MultiBuffer 解析到最近邻文件。
    pub fn restore_scroll_anchor(
        &mut self,
        anchor: EditorScrollAnchor,
        cx: &mut Context<Self>,
    ) -> bool {
        self.sync_display_map(cx);
        let Some(output_offset) = self
            .multi_buffer
            .read(cx)
            .resolve_anchor(&anchor.buffer_anchor)
        else {
            return false;
        };
        let Ok(canonical_point) = self.display_map.offset_to_display_point(output_offset) else {
            return false;
        };
        let display_point = DisplayPoint::new(
            DisplayRow::new(
                canonical_point
                    .row()
                    .get()
                    .saturating_sub(anchor.preceding_virtual_rows),
            ),
            canonical_point.column(),
        );
        if self
            .scroll_manager
            .restore_anchor(display_point, anchor.offset)
        {
            self.input_layout = None;
            cx.notify();
        }
        true
    }

    pub(super) fn longest_display_row(&self) -> DisplayRow {
        self.display_map.longest_measured_row()
    }

    pub(super) fn measure_display_rows(&mut self, start: DisplayRow, line_count: usize) {
        if let Err(error) = self.display_map.measure_rows(start, line_count) {
            eprintln!("Editor 测量显示行失败：{error}");
        }
    }

    pub(super) fn select_line(&mut self, line: Line, extend: bool) {
        let snapshot = self.render_snapshot();
        let Ok(start) = snapshot.line_start_byte(line) else {
            return;
        };
        let end = snapshot
            .line_start_byte(Line::new(line.get() + 1))
            .unwrap_or_else(|_| snapshot.len_bytes());
        let selection = if extend {
            let current = *self.selections.resolve(&snapshot).primary();
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
        self.set_selections(SelectionSet::new(vec![selection]));
        self.request_autoscroll();
    }

    pub(super) fn set_input_layout(&mut self, layout: EditorInputLayout) {
        self.input_layout = Some(layout);
    }

    /// 鼠标左键按下：按点击次数开始选区手势，并记录拖拽起点。
    ///
    /// 单击定位光标、双击选中词、三击选中整行、四击及以上全选；
    /// `extend`（Shift 按下）时按上次手势粒度扩展选区。
    pub(super) fn begin_selection(
        &mut self,
        offset: ByteOffset,
        click_count: usize,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        let buffer_entity = self.text_buffer(cx);
        let buffer = buffer_entity.read(cx);
        let Ok(char_offset) = buffer.byte_to_char(offset) else {
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

        // 本次点击粒度对应的候选范围；坐标计算失败时放弃手势。
        let (start, end, mode) = match click_count {
            1 => (offset, offset, MouseSelectMode::Character),
            2 => {
                let Ok((word_start, word_end)) = buffer.surrounding_word(char_offset) else {
                    return;
                };
                let Ok(word_start) = buffer.char_to_byte(word_start) else {
                    return;
                };
                let Ok(word_end) = buffer.char_to_byte(word_end) else {
                    return;
                };
                (
                    word_start,
                    word_end,
                    MouseSelectMode::Word(word_start..word_end),
                )
            }
            3 => {
                let Ok(line) = buffer.byte_to_line(offset) else {
                    return;
                };
                let Ok(line_start) = buffer.line_start_byte(line) else {
                    return;
                };
                let line_end = buffer
                    .line_start_byte(Line::new(line.get() + 1))
                    .unwrap_or(buffer.len_bytes());
                (
                    line_start,
                    line_end,
                    MouseSelectMode::Line(line_start..line_end),
                )
            }
            _ => (ByteOffset::ZERO, buffer.len_bytes(), MouseSelectMode::All),
        };

        // Shift+点击：以上次选区锚点为固定端，按点击位置向两侧扩展；点击范围覆盖锚点时整段纳入。
        let selection = if extend {
            let tail = self.resolved_selections().primary().anchor();
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
        self.set_pending_selection(SelectionSet::new(vec![selection]));
        self.request_autoscroll();
        self.input_layout = None;
        self.pending_selection = Some(PendingSelection {
            anchor: offset,
            mode,
        });
        cx.notify();
    }

    /// 鼠标拖动：按按下时的粒度把选区活动端更新到当前位置。
    ///
    /// 词/行粒度下按整词/整行边界吸附，避免半词截断。
    pub(super) fn update_selection(&mut self, offset: ByteOffset, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_selection.clone() else {
            return;
        };
        let buffer_entity = self.text_buffer(cx);
        let buffer = buffer_entity.read(cx);
        let Ok(char_offset) = buffer.byte_to_char(offset) else {
            return;
        };
        let (head, tail) = match pending.mode {
            MouseSelectMode::Character => (offset, pending.anchor),
            MouseSelectMode::Word(original_range) => {
                // 光标仍在词内（或落在原词范围内）时按整词边界吸附，head 取点击侧的词端。
                let inside = buffer.is_inside_word(char_offset).unwrap_or(false)
                    || original_range.contains(&offset);
                let head = if inside {
                    let Ok((word_start, word_end)) = buffer.surrounding_word(char_offset) else {
                        return;
                    };
                    let Ok(word_start) = buffer.char_to_byte(word_start) else {
                        return;
                    };
                    let Ok(word_end) = buffer.char_to_byte(word_end) else {
                        return;
                    };
                    if word_start < original_range.start {
                        word_start
                    } else {
                        word_end
                    }
                } else {
                    offset
                };
                // 活动端在原词左侧时锚定原词右端，否则锚定左端。
                if head <= original_range.start {
                    (head, original_range.end)
                } else {
                    (head, original_range.start)
                }
            }
            MouseSelectMode::Line(original_range) => {
                // 行粒度：head 所在整行纳入（含行尾换行符）。
                let Ok(line) = buffer.byte_to_line(offset) else {
                    return;
                };
                let Ok(line_start) = buffer.line_start_byte(line) else {
                    return;
                };
                let next_line_start = buffer
                    .line_start_byte(Line::new(line.get() + 1))
                    .unwrap_or(buffer.len_bytes());
                let head = if line_start < original_range.start {
                    line_start
                } else {
                    next_line_start
                };
                if head <= original_range.start {
                    (head, original_range.end)
                } else {
                    (head, original_range.start)
                }
            }
            MouseSelectMode::All => return,
        };
        self.composition = None;
        self.set_pending_selection(SelectionSet::new(vec![Selection::new(tail, head)]));
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
    ) {
        self.scroll_manager.update_viewport(
            self.display_map.line_count(),
            viewport_size.width,
            viewport_size.height,
            content_width,
            line_height,
            top_inset,
        );
    }

    pub(super) fn scroll_by(&mut self, delta: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        if self.scroll_manager.scroll_by(delta) {
            self.input_layout = None;
            cx.notify();
            true
        } else {
            false
        }
    }

    /// 布局前消费待自动滚动点并应用垂直部分（见 `ScrollManager::apply_pending_autoscroll_vertical`）。
    ///
    /// 目标显示点按当前布局换算：软换行宽度在此帧已确定，换算出的行号与最终布局一致，避免导航请求在换行重排前固化错误的目标行。
    pub(super) fn apply_pending_autoscroll_vertical(&mut self) -> bool {
        self.scroll_manager
            .apply_pending_autoscroll_vertical(|head| {
                self.display_map.offset_to_display_point(head).ok()
            })
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
        if self.scroll_manager.scroll_to(scroll_top) {
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
        let initial_version = snapshot.text().version();
        let display_map = DisplayMap::new(snapshot);
        cx.observe(&multi_buffer, |editor, multi_buffer, cx| {
            let dirty = multi_buffer.read(cx).is_dirty(cx);
            if editor.last_dirty != dirty {
                editor.last_dirty = dirty;
                cx.emit(EditorEvent::DirtyChanged);
            }
            cx.notify();
        })
        .detach();
        cx.subscribe(&multi_buffer, |editor, _, event, cx| {
            match event {
                MultiBufferEvent::TextChanged => editor.sync_display_map(cx),
                MultiBufferEvent::Reparsed => {
                    editor.sync_display_map(cx);
                    editor.refresh_fold_ranges(cx);
                }
                MultiBufferEvent::MetadataChanged => {}
                MultiBufferEvent::DiffExpansionChanged => {
                    cx.emit(EditorEvent::DiffHunksExpandedChanged);
                }
            }
            editor.input_layout = None;
            cx.notify();
        })
        .detach();

        let blink_manager = cx.new(|_| BlinkManager::new());
        cx.observe(&blink_manager, |_, _, cx| cx.notify()).detach();

        // 换行模式默认来自全局设置，与编辑器模式无关；
        // UI 场景可用 set_soft_wrap_mode 覆盖（覆盖存在时设置变化不生效）。
        let settings = SettingsStore::try_get(cx);
        let mut this = Self {
            multi_buffer,
            multi_buffer_subscription,
            last_dirty,
            display_map,
            mode,
            placeholder_display_map: None,
            selections: EditorSelections::from_selection_set(
                initial_version,
                &SelectionSet::default(),
            ),
            selection_history: SelectionHistory::default(),
            fold_ranges: Arc::from([]),
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
            mouse_select_mode: MouseSelectMode::Character,
            pending_selection: None,
            autoclose_regions: Vec::new(),
        };
        // 设置变化时自动跟随（覆盖场景除外）；编辑器在测试环境无 SettingsStore 时保持默认。
        cx.observe_global::<SettingsStore>(|editor, cx| {
            if editor.soft_wrap_override.is_some() {
                return;
            }
            if let Some(settings) = SettingsStore::try_get(cx) {
                editor.soft_wrap = settings.soft_wrap.into();
                editor.preferred_line_length = settings.preferred_line_length;
                cx.notify();
            }
        })
        .detach();
        this.refresh_fold_ranges(cx);
        this
    }

    /// 提交一次文本编辑事务的唯一入口。
    ///
    /// 会话模型：入口统一负责会话开启/提交（`start_transaction` / `end_transaction`）、Buffer 通知、编辑后选区锚点映射、SelectionHistory 记录、display_map 同步与搜索重搜（`apply_edit_outcome` 全链路）。
    /// 返回编辑结果供需要事务身份的调用方消费（如 IME 组合会话）；失败时错误已打印、选区已恢复，调用方只需处理自身特判状态。
    pub(super) fn change(
        &mut self,
        before_selections: SelectionSet,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Buffer) -> TextResult<EditOutcome>,
    ) -> TextResult<EditOutcome> {
        let (node_id, outcome) = self.commit_session(before_selections, cx, f)?;
        self.apply_edit_outcome(node_id, outcome, cx)
    }

    /// 编辑后选区由闭包按编辑语义重算的变体（删除、剪切、行移动、输入等特判场景）。
    pub(super) fn change_with_after(
        &mut self,
        before_selections: SelectionSet,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Buffer) -> TextResult<(EditOutcome, SelectionSet)>,
    ) -> TextResult<(EditOutcome, SelectionSet)> {
        let (node_id, outcome) = self.commit_session(before_selections, cx, f)?;
        self.apply_edit_outcome_with_after(node_id, outcome, cx)
    }

    /// 会话化编辑的共享骨架：开启会话并记录 undo 选区（事务开始时记录）→ 闭包编辑（统一 Buffer 通知）→ 提交会话，返回 (节点身份, 编辑结果)。
    ///
    /// 编辑失败时结束空会话（不产生历史节点）、恢复编辑前选区并回传错误；合并进前节点时清理会话自身的孤儿选区记录。
    fn commit_session<T>(
        &mut self,
        before_selections: SelectionSet,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Buffer) -> TextResult<T>,
    ) -> TextResult<(Option<TransactionId>, T)> {
        let session_id = self.start_transaction(before_selections.clone(), cx)?;
        if self.multi_buffer.read(cx).is_composite() {
            let projection_snapshot = self.text_buffer(cx).read(cx).snapshot();
            let projection_text = projection_snapshot
                .slice_text(TextRange::new(
                    ByteOffset::ZERO,
                    projection_snapshot.len_bytes(),
                )?)?
                .as_str()
                .to_owned();
            let mut planner =
                Buffer::scratch(projection_text, projection_snapshot.config().clone())?;
            let subscription = planner.subscribe();
            let outcome = match f(&mut planner) {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.end_transaction(cx);
                    eprintln!("Editor 编辑事务失败：{error}");
                    self.selections = EditorSelections::from_selection_set(
                        projection_snapshot.version(),
                        &before_selections,
                    );
                    return Err(error);
                }
            };
            let changes = subscription.consume();
            // 编辑映射与提交可能失败（如命中只读 excerpt）：失败必须结束空会话并恢复编辑前选区，否则事务残留会阻塞后续所有编辑。
            let applied = (|| -> TextResult<()> {
                if !changes.is_empty() {
                    let after = planner.snapshot();
                    let edits = changes
                        .patch()
                        .edits()
                        .iter()
                        .map(|patch| {
                            let replacement =
                                after.slice_text(patch.new_range())?.as_str().to_owned();
                            Ok(Edit::replace(patch.old_range(), replacement))
                        })
                        .collect::<TextResult<Vec<_>>>()?;
                    self.multi_buffer.update(cx, |buffer, cx| {
                        buffer.edit(
                            edits,
                            TransactionMetadata::new(TransactionSource::Programmatic)
                                .with_description("MultiBuffer 编辑"),
                            cx,
                        )
                    })?;
                }
                Ok(())
            })();
            if let Err(error) = applied {
                self.end_transaction(cx);
                eprintln!("Editor 编辑事务失败：{error}");
                let version = self.text_buffer(cx).read(cx).snapshot().version();
                self.selections = EditorSelections::from_selection_set(version, &before_selections);
                return Err(error);
            }
            let node_id = self.end_transaction(cx);
            if node_id != Some(session_id) {
                self.selection_history.remove_transaction(session_id);
            }
            return Ok((node_id, outcome));
        }
        let buffer = self.text_buffer(cx);
        let outcome = buffer.update(cx, |buffer, cx| {
            let outcome = f(buffer)?;
            cx.notify();
            Ok(outcome)
        });
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.end_transaction(cx);
                eprintln!("Editor 编辑事务失败：{error}");
                let version = self.text_buffer(cx).read(cx).snapshot().version();
                self.selections = EditorSelections::from_selection_set(version, &before_selections);
                return Err(error);
            }
        };
        let node_id = self.end_transaction(cx);
        if node_id != Some(session_id) {
            self.selection_history.remove_transaction(session_id);
        }
        Ok((node_id, outcome))
    }

    /// 开启编辑会话并记录 undo 选区。
    ///
    /// Editor 不嵌套会话：zcv-text 会话已开启时视为内部错误。
    fn start_transaction(
        &mut self,
        undo_selections: SelectionSet,
        cx: &mut Context<Self>,
    ) -> TextResult<TransactionId> {
        let transaction_id = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.start_transaction(cx))?;
        self.selection_history
            .insert_transaction(transaction_id, undo_selections);
        Ok(transaction_id)
    }

    /// 提交编辑会话；会话内无编辑时返回 `None`（编辑失败路径）。
    fn end_transaction(&mut self, cx: &mut Context<Self>) -> Option<TransactionId> {
        self.multi_buffer
            .update(cx, |buffer, cx| buffer.end_transaction(cx))
    }

    /// 编辑事务结果落位：选区锚点映射、redo 选区记录、display_map 同步与搜索重搜。
    ///
    /// 会话提交后的历史节点身份（与本次编辑的事件身份分离，合并进前节点时指向被合并的既有节点）；
    /// `None` 表示空会话或历史被预算清空，此时不记录选区历史。
    /// 编辑失败路径由 `change` 统一处理，这里只消费成功结果。
    fn apply_edit_outcome(
        &mut self,
        transaction_id: Option<TransactionId>,
        outcome: EditOutcome,
        cx: &mut Context<Self>,
    ) -> TextResult<EditOutcome> {
        if let Some(transaction) = outcome.transaction() {
            if self.multi_buffer.read(cx).is_composite() {
                let version = self.text_buffer(cx).read(cx).snapshot().version();
                self.update_autoclose_regions_with(
                    transaction.event().position_map(),
                    self.selections.version(),
                    version,
                );
            } else {
                self.update_autoclose_regions(transaction.event());
            }
            // 用本次事务的坐标映射批量推进选区端点锚点，选区自动跟随文本变化。
            let snapshot = self.text_buffer(cx).read(cx).snapshot();
            let new_version = snapshot.version();
            let position_map = transaction.event().position_map();
            self.selections.map_through_position_map(
                self.selections.version(),
                new_version,
                position_map,
            );
            if let Some(transaction_id) = transaction_id
                && let Some(transaction) = self.selection_history.transaction_mut(transaction_id)
            {
                // display_map 尚未同步到新版本，历史快照按 Buffer 快照解析。
                // 事务结束时记录 redo 选区。
                let after_selections = self.selections.resolve(&snapshot);
                transaction.set_redo(after_selections);
            }
        }
        self.finish_edit(cx);
        self.research_after_edit(cx);
        cx.emit(EditorEvent::Edited);
        Ok(outcome)
    }

    /// 行移动等特判场景：编辑后选区按行语义重算，直接重锚定结果，不走通用锚点映射。
    fn apply_edit_outcome_with_after(
        &mut self,
        transaction_id: Option<TransactionId>,
        outcome: (EditOutcome, SelectionSet),
        cx: &mut Context<Self>,
    ) -> TextResult<(EditOutcome, SelectionSet)> {
        let (outcome, after_selections) = outcome;
        if let Some(transaction) = outcome.transaction() {
            if self.multi_buffer.read(cx).is_composite() {
                let version = self.text_buffer(cx).read(cx).snapshot().version();
                self.update_autoclose_regions_with(
                    transaction.event().position_map(),
                    self.selections.version(),
                    version,
                );
            } else {
                self.update_autoclose_regions(transaction.event());
            }
        }
        if let Some(transaction_id) = transaction_id
            && let Some(transaction) = self.selection_history.transaction_mut(transaction_id)
        {
            transaction.set_redo(after_selections.clone());
        }
        // 编辑后 display_map 尚未同步，重锚定用 Buffer 快照的当前版本。
        let version = self.text_buffer(cx).read(cx).snapshot().version();
        self.selections = EditorSelections::from_selection_set(version, &after_selections);
        self.finish_edit(cx);
        self.research_after_edit(cx);
        cx.emit(EditorEvent::Edited);
        Ok((outcome, after_selections))
    }

    fn finish_edit(&mut self, cx: &mut Context<Self>) {
        self.pending_selection = None;
        self.sync_display_map(cx);
        self.request_autoscroll();
        self.input_layout = None;
        self.blink_manager.update(cx, |blink, cx| {
            blink.pause_blinking(cx);
        });
        cx.notify();
    }

    /// 将自动闭合区域随一次文本变更推进到新版本。
    ///
    /// 区域版本与变更起点失配时整体清空（说明存在未走编辑入口的文本变更，陈旧区域坐标已不可信，继续保留会误触发跳过/删对）。
    fn update_autoclose_regions(&mut self, event: &DeltaEvent) {
        self.update_autoclose_regions_with(
            event.position_map(),
            event.old_version(),
            event.new_version(),
        );
    }

    fn update_autoclose_regions_with(
        &mut self,
        position_map: &PositionMap,
        old_version: BufferVersion,
        new_version: BufferVersion,
    ) {
        let mut kept = Vec::with_capacity(self.autoclose_regions.len());
        for region in std::mem::take(&mut self.autoclose_regions) {
            if region.range.version() != old_version {
                continue;
            }
            // 映射结果一律保留（Anchor 语义：删除内容不使锚失效）：
            // 区域锚在闭合符起点，闭合符是否存活由使用处的文本校验兜底。
            let range = region
                .range
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
        let selections = self.resolved_selections();
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
                        if unit == MovementUnit::LineEdge && self.display_map.is_wrapped() {
                            let head = selection.head();
                            return match direction {
                                MovementDirection::Previous => self
                                    .display_map
                                    .beginning_of_row(head)
                                    .map_err(|error| zcv_text::TextError::InvariantViolation {
                                        location: "Editor::move_selections",
                                        detail: error.to_string(),
                                    }),
                                MovementDirection::Next => self
                                    .display_map
                                    .end_of_row(head)
                                    .map_err(|error| zcv_text::TextError::InvariantViolation {
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
                        let buffer_entity = self.text_buffer(cx);
                        let buffer = buffer_entity.read(cx);
                        let head = buffer.byte_to_char(base)?;
                        let target = buffer.movement_boundary(head, direction, unit)?;
                        let mut target = buffer.char_to_byte(target)?;
                        // 折叠感知：目标落在折叠内时按方向吸附到折叠终点/起点。
                        // 折叠在显示上占一个字符（合并行占位符），水平移动一步跨过。
                        if let Some((start, end)) = self
                            .display_map
                            .snapshot()
                            .fold_range_covering_offset(target)
                        {
                            target = match direction {
                                MovementDirection::Next => end,
                                MovementDirection::Previous => start,
                            };
                        }
                        target
                    }
                    Motion::LineStep | Motion::PageStep(_) => {
                        let row_step = match motion {
                            Motion::PageStep(row_step) => row_step,
                            _ => 1,
                        };
                        let point =
                            self.display_map
                                .offset_to_display_point(base)
                                .map_err(|error| zcv_text::TextError::InvariantViolation {
                                    location: "Editor::move_selections",
                                    detail: error.to_string(),
                                })?;
                        // 目标列：优先使用持久化的 goal，否则从当前位置推导。
                        let goal = selection
                            .goal()
                            .map(DisplayColumn::new)
                            .unwrap_or(point.column());
                        vertical_goal = Some(goal);
                        let last_row = self.display_map.line_count().saturating_sub(1);
                        if direction == MovementDirection::Previous
                            && point.row() == DisplayRow::ZERO
                        {
                            return Ok(if extend {
                                selection
                                    .with_head(ByteOffset::ZERO)
                                    .with_goal(Some(goal.get()))
                            } else {
                                Selection::caret(ByteOffset::ZERO).with_goal(Some(goal.get()))
                            });
                        }
                        if direction == MovementDirection::Next && point.row().get() >= last_row {
                            let new_head = self.display_map.buffer_snapshot().len_bytes();
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
                        self.display_map
                            .display_point_to_offset(DisplayPoint::new(
                                DisplayRow::new(target_row),
                                goal,
                            ))
                            .map_err(|error| zcv_text::TextError::InvariantViolation {
                                location: "Editor::move_selections",
                                detail: error.to_string(),
                            })?
                    }
                    Motion::DocumentEdge => match direction {
                        MovementDirection::Previous => ByteOffset::ZERO,
                        MovementDirection::Next => self.text_buffer(cx).read(cx).len_bytes(),
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
                let version = self.display_map.buffer_snapshot().version();
                self.selections = EditorSelections::from_selection_set(version, &selections);
                if matches!(motion, Motion::PageStep(_)) {
                    self.scroll_manager
                        .scroll_page(direction == MovementDirection::Next);
                }
                self.request_autoscroll();
                self.input_layout = None;
                self.blink_manager.update(cx, |blink, cx| {
                    blink.pause_blinking(cx);
                });
                cx.notify();
            }
            Err(error) => eprintln!("Editor 选区移动失败：{error}"),
        }
    }

    fn request_autoscroll(&mut self) {
        let head = self.resolved_selections().primary().head();
        self.scroll_manager.request_autoscroll(head);
    }

    /// 导航跳转定位：把光标行固定在视口顶部下方指定行数。
    pub(super) fn request_scroll_to_top(&mut self, offset_rows: usize) {
        let head = self.resolved_selections().primary().head();
        self.scroll_manager.request_scroll_to_top(head, offset_rows);
    }

    fn sync_display_map(&mut self, cx: &App) {
        let snapshot = self.multi_buffer.read(cx).snapshot(cx);
        let text_version = snapshot.text().version();
        let changes = self.multi_buffer_subscription.consume();
        if changes.is_empty() {
            self.display_map.sync(snapshot, changes);
            return;
        }
        if changes.requires_reset() {
            // 整体替换（外部加载）：锚点无法映射，选区回落到文档开头，由宿主随后重设。
            self.selections =
                EditorSelections::from_selection_set(text_version, &SelectionSet::default());
        } else if let Some(old_version) = changes.old_version() {
            // 共享 Buffer 的其他 Editor 或 zcv-text 直接编辑：批量映射端点锚点。
            // 本 Editor 自己发起的编辑已在 apply_edit_outcome 映射过，版本已推进，跳过。
            if old_version == self.selections.version() {
                let position_map = PositionMap::from_text_patch(changes.patch());
                self.selections
                    .map_through_position_map(old_version, text_version, &position_map);
            }
        }
        self.display_map.sync(snapshot, changes);
        // 折叠范围只在组合快照的语法版本更新后刷新：
        // 编辑时立即全量查询既在主线程跑 O(N) fold 查询，又会因版本不匹配把折叠清空。
    }

    /// 读取共享 LanguageBuffer 的折叠缓存（后台解析时已计算，主线程零查询）。
    ///
    /// 缓存只在 Reparsed 安装后整体替换；文本已编辑但新解析未安装的窗口期保留上一版结果。
    fn refresh_fold_ranges(&mut self, cx: &App) {
        self.fold_ranges = self.multi_buffer.read(cx).fold_ranges(cx);
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
        let line_count = self.display_map.buffer_snapshot().line_count();
        if let Ok(line_range) = LineRange::new(Line::ZERO, Line::new(line_count))
            && let Err(error) = self.display_map.unfold_lines(line_range)
        {
            eprintln!("展开折叠失败：{error}");
        }
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
        let end = self.text_buffer(cx).read(cx).len_bytes();
        self.change_selections(
            SelectionSet::new(vec![Selection::new(ByteOffset::ZERO, end)]),
            cx,
        );
    }

    pub(super) fn handle_expand_selection(
        &mut self,
        _: &ExpandSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self.text_buffer(cx).read(cx).snapshot();
        let expanded = self
            .resolved_selections()
            .as_slice()
            .iter()
            .map(|selection| {
                let range = selection.start().get()..selection.end().get();
                self.display_map
                    .syntax_snapshot()
                    .ancestor_range(range, &snapshot)
                    .map(|range| {
                        Selection::new(ByteOffset::new(range.start), ByteOffset::new(range.end))
                    })
                    .unwrap_or(*selection)
            })
            .collect();
        self.change_selections(
            SelectionSet::new_with_primary(expanded, self.resolved_selections().primary_index()),
            cx,
        );
    }
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

        self.sync_display_map(cx);

        // SingleLine / AutoHeight 用于搜索框等 UI 场景，应使用 UI 字号而非编辑器字号
        let (font, text_size, line_height) = match self.mode {
            EditorMode::SingleLine | EditorMode::AutoHeight { .. } => (
                typography::ui_font(),
                typography::ui(),
                typography::ui_line(),
            ),
            EditorMode::Full => (
                typography::editor_font(),
                typography::editor(),
                typography::editor_line(),
            ),
        };
        let visible_lines = match self.mode {
            EditorMode::SingleLine => Some(1),
            EditorMode::AutoHeight {
                min_lines,
                max_lines,
            } => {
                let line_count = self.display_map.line_count().max(min_lines);
                Some(max_lines.map_or(line_count, |maximum| line_count.min(maximum)))
            }
            EditorMode::Full => None,
        };

        EditorElement::register_actions(
            div()
                .track_focus(&self.focus)
                .key_context("Editor")
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
                .text_color(color::current(cx).text),
            cx,
        )
        .child(EditorElement::new(cx.entity()))
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

use editing::touched_lines;
use input::AutocloseRegion;
/// 输入法组合会话与展示快照（element 渲染 marked ranges 用）。
pub(super) use input::{EditorComposition, EditorPresentation};

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

/// 编辑事务元数据（供编辑命令与输入共用）。
pub(super) fn edit_metadata(description: &'static str) -> TransactionMetadata {
    TransactionMetadata::new(TransactionSource::Programmatic).with_description(description)
}
