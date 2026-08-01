//! Editor View 的跨帧状态与交互。

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, IntoElement, Pixels, Point, Render, Styled, UTF16Selection, Window, actions, div,
    point, prelude::*, px, size,
};
use zcv_engine::{
    Buffer, BufferConfig, ByteOffset, EngineResult, Line, MovementDirection, MovementUnit,
    Selection, SelectionSet, Snapshot, TextRange, TextSubscription, TransactionId,
    TransactionMergePolicy, TransactionMetadata, TransactionSource, Utf16Offset,
};
use zcv_language::{BracketPair, LanguageBuffer, SyntaxSnapshot};

use super::blink_manager::BlinkManager;
use super::display_map::{DisplayMap, DisplayPoint, DisplayRow, DisplaySnapshot};
use super::element::{EditorElement, EditorInputLayout};
use super::scroll::ScrollManager;
use super::selection::{EditOutcome, SelectionHistory, apply_targeted_edits, replace_selections};
use zcv_theme::{color, typography};

actions!(
    editor,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        MoveToPreviousWord,
        MoveToNextWord,
        MoveToBeginningOfLine,
        MoveToEndOfLine,
        MoveToBeginning,
        MoveToEnd,
        MovePageUp,
        MovePageDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectToPreviousWord,
        SelectToNextWord,
        SelectToBeginningOfLine,
        SelectToEndOfLine,
        SelectToBeginning,
        SelectToEnd,
        SelectPageUp,
        SelectPageDown,
        SelectAll,
        ExpandSelection,
        Backspace,
        Delete,
        DeleteToPreviousWordStart,
        DeleteToNextWordEnd,
        DeleteToBeginningOfLine,
        DeleteToEndOfLine,
        Newline,
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        Indent,
        Outdent,
    ]
);

/// Editor 自身的领域事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    /// 编辑器关联的文件路径发生变化。
    PathChanged,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditorMode {
    SingleLine,
    AutoHeight {
        min_lines: usize,
        max_lines: Option<usize>,
    },
    Full,
}

/// 软换行模式，与 Zed 的 soft_wrap 设置语义一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftWrap {
    /// 不换行（超长行横向滚动）。
    None,
    /// 超过编辑器文本区宽度换行。
    EditorWidth,
    /// 在 `preferred_line_length` 与编辑器宽度（取小者）处换行。
    Bounded,
}

impl Default for SoftWrap {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EditorComposition {
    ranges: Arc<[TextRange]>,
    primary_index: usize,
    history_transaction_id: Option<TransactionId>,
}

#[derive(Debug, Clone)]
pub(super) struct EditorPresentation {
    snapshot: Snapshot,
    composition: Option<EditorComposition>,
}

impl EditorPresentation {
    pub(super) fn new(snapshot: &Snapshot, composition: Option<&EditorComposition>) -> Self {
        Self {
            snapshot: snapshot.clone(),
            composition: composition.cloned(),
        }
    }

    pub(super) fn marked_ranges(&self) -> &[TextRange] {
        self.composition
            .as_ref()
            .map_or(&[], |composition| composition.ranges.as_ref())
    }

    pub(super) fn marked_utf16_range(&self) -> Option<Range<usize>> {
        let composition = self.composition.as_ref()?;
        let range = composition.ranges.get(composition.primary_index)?;
        Some(
            self.snapshot.byte_to_utf16_cu(range.start()).ok()?.get()
                ..self.snapshot.byte_to_utf16_cu(range.end()).ok()?.get(),
        )
    }

    fn text_for_utf16_range(&self, range: Range<usize>) -> Option<String> {
        let start = self
            .snapshot
            .utf16_cu_to_byte(Utf16Offset::new(range.start))
            .ok()?;
        let end = self
            .snapshot
            .utf16_cu_to_byte(Utf16Offset::new(range.end))
            .ok()?;
        self.snapshot
            .slice_byte_range(start, end)
            .ok()
            .map(|text| text.as_str().to_owned())
    }
}

pub struct Editor {
    language_buffer: Entity<LanguageBuffer>,
    buffer: Entity<Buffer>,
    buffer_subscription: TextSubscription,
    display_map: DisplayMap,
    syntax_snapshot: SyntaxSnapshot,
    mode: EditorMode,
    project_root: Option<PathBuf>,
    selections: SelectionSet,
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
    soft_wrap: SoftWrap,
    preferred_line_length: usize,
}

impl Editor {
    pub fn single_line(cx: &mut Context<Self>) -> Self {
        let buffer = Buffer::scratch(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");
        let buffer = cx.new(|_| buffer);
        let language_buffer = cx.new(|cx| LanguageBuffer::new(buffer, None, cx));
        Self::new(language_buffer, EditorMode::SingleLine, cx)
    }

    pub fn auto_height(min_lines: usize, max_lines: Option<usize>, cx: &mut Context<Self>) -> Self {
        let buffer = Buffer::scratch(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");
        let buffer = cx.new(|_| buffer);
        let language_buffer = cx.new(|cx| LanguageBuffer::new(buffer, None, cx));
        Self::new(
            language_buffer,
            EditorMode::AutoHeight {
                min_lines,
                max_lines,
            },
            cx,
        )
    }

    pub fn for_buffer(language_buffer: Entity<LanguageBuffer>, cx: &mut Context<Self>) -> Self {
        Self::new(language_buffer, EditorMode::Full, cx)
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 光标是否应当绘制。
    ///
    /// 编辑器未聚焦时不显示；聚焦时由 BlinkManager 控制闪烁。
    pub(crate) fn show_local_cursors(&self, window: &Window, cx: &App) -> bool {
        self.blink_manager.read(cx).visible() && self.focus.is_focused(window)
    }

    pub fn buffer(&self) -> Entity<Buffer> {
        self.buffer.clone()
    }

    /// 设置软换行模式与 preferred_line_length；实际换行在下一帧 prepaint
    /// 计算 wrap 宽度时生效。
    pub fn set_soft_wrap(
        &mut self,
        soft_wrap: SoftWrap,
        preferred_line_length: usize,
        cx: &mut Context<Self>,
    ) {
        if self.soft_wrap == soft_wrap && self.preferred_line_length == preferred_line_length {
            return;
        }
        self.soft_wrap = soft_wrap;
        self.preferred_line_length = preferred_line_length;
        cx.notify();
    }

    pub(crate) fn soft_wrap(&self) -> SoftWrap {
        self.soft_wrap
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
            .set_wrap_width(wrap_width, font, font_size, &text_system);
    }

    pub fn file_path(&self, cx: &App) -> Option<PathBuf> {
        self.language_buffer
            .read(cx)
            .file_path()
            .map(Path::to_path_buf)
    }

    pub fn language_name(&self, cx: &App) -> Option<&'static str> {
        self.language_buffer.read(cx).language_name()
    }

    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    pub fn set_file_path(&mut self, path: PathBuf, project_root: PathBuf, cx: &mut Context<Self>) {
        self.language_buffer.update(cx, |language_buffer, cx| {
            language_buffer.set_file_path(path, cx)
        });
        self.project_root = Some(project_root);
        cx.emit(EditorEvent::PathChanged);
    }

    pub fn text(&self, cx: &App) -> String {
        let snapshot = self.buffer.read(cx).snapshot();
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .expect("完整 Editor Snapshot 范围必须可读取")
            .as_str()
            .to_owned()
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        self.buffer.read(cx).is_dirty()
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.composition = None;
        let before_selections = self.selections.clone();
        let targets = SelectionSet::new(vec![Selection::new(
            ByteOffset::ZERO,
            self.buffer.read(cx).len_bytes(),
        )]);
        let text = if self.mode == EditorMode::SingleLine {
            text.replace(['\r', '\n'], "")
        } else {
            text.to_owned()
        };
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = replace_selections(buffer, &targets, &text, edit_metadata("设置文本"));
            cx.notify();
            outcome
        });
        self.apply_edit_outcome(before_selections, outcome, cx);
    }

    /// 将单个选择区设置为给定的 UTF-8 字节范围。
    pub fn select_byte_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let end = self.buffer.read(cx).len_bytes();
        assert!(range.start <= range.end && ByteOffset::new(range.end) <= end);
        self.composition = None;
        self.selections = SelectionSet::new(vec![Selection::new(
            ByteOffset::new(range.start),
            ByteOffset::new(range.end),
        )]);
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

    pub(super) fn syntax_snapshot(&self) -> SyntaxSnapshot {
        self.syntax_snapshot.clone()
    }

    pub(super) fn matching_bracket_pair(&self) -> Option<BracketPair> {
        let snapshot = self.display_map.buffer_snapshot();
        let caret = self.selections.primary().head().get();
        let start = caret.saturating_sub(1);
        let end = caret.saturating_add(1).min(snapshot.len_bytes().get());
        self.syntax_snapshot
            .bracket_pairs(start..end, snapshot)
            .into_iter()
            .find(|pair| {
                [
                    pair.open.start,
                    pair.open.end,
                    pair.close.start,
                    pair.close.end,
                ]
                .contains(&caret)
            })
    }

    pub fn selections(&self) -> SelectionSet {
        self.selections.clone()
    }

    /// 光标位置的 "行:列" 文本，行和列均从 1 开始计数。
    pub fn cursor_text(&self) -> String {
        let point = self
            .display_map
            .buffer_snapshot()
            .byte_to_position(self.selections.primary().head());
        match point {
            Ok(p) => format!("{}:{}", p.line().get() + 1, p.column().get() + 1),
            Err(_) => String::new(),
        }
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
        touched_lines(self.display_map.buffer_snapshot(), &self.selections).unwrap_or_default()
    }

    pub(super) fn scroll_anchor(&self) -> DisplayPoint {
        self.scroll_manager.anchor()
    }

    pub(super) fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll_manager.offset()
    }

    pub(super) fn longest_display_row(&self) -> DisplayRow {
        self.display_map.longest_measured_row()
    }

    pub(super) fn measure_display_rows(&mut self, start: DisplayRow, line_count: usize) {
        if let Err(error) = self.display_map.measure_rows(start, line_count) {
            eprintln!("Editor 测量显示行失败：{error}");
        }
    }

    pub(super) fn set_caret(&mut self, offset: ByteOffset) {
        self.composition = None;
        self.selections = SelectionSet::caret(offset);
        self.request_autoscroll();
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
            let current = *self.selections.primary();
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
        self.selections = SelectionSet::new(vec![selection]);
        self.request_autoscroll();
    }

    pub(super) fn set_input_layout(&mut self, layout: EditorInputLayout) {
        self.input_layout = Some(layout);
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
    ) {
        self.scroll_manager.update_viewport(
            self.display_map.line_count(),
            viewport_size.width,
            viewport_size.height,
            content_width,
            line_height,
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

    pub(super) fn complete_autoscroll(
        &mut self,
        caret_left: Option<Pixels>,
        caret_right: Option<Pixels>,
    ) -> bool {
        self.scroll_manager
            .complete_autoscroll(caret_left, caret_right)
    }

    fn new(
        language_buffer: Entity<LanguageBuffer>,
        mode: EditorMode,
        cx: &mut Context<Self>,
    ) -> Self {
        let buffer = language_buffer.read(cx).buffer();
        // 在一次 Entity 更新中建立订阅并取得同版本 Snapshot，关闭初始化期间的漏读窗口。
        let (buffer_subscription, snapshot) =
            buffer.update(cx, |buffer, _| (buffer.subscribe(), buffer.snapshot()));
        let syntax_snapshot = language_buffer.read(cx).syntax_snapshot();
        let display_map = DisplayMap::new(snapshot);
        cx.observe(&language_buffer, |editor, language_buffer, cx| {
            editor.syntax_snapshot = language_buffer.read(cx).syntax_snapshot();
            editor.sync_display_map(cx);
            editor.input_layout = None;
            cx.notify();
        })
        .detach();

        let blink_manager = cx.new(|_| BlinkManager::new());
        cx.observe(&blink_manager, |_, _, cx| cx.notify()).detach();

        Self {
            language_buffer,
            buffer,
            buffer_subscription,
            display_map,
            syntax_snapshot,
            mode,
            project_root: None,
            selections: SelectionSet::default(),
            selection_history: SelectionHistory::default(),
            scroll_manager: ScrollManager::default(),
            composition: None,
            input_layout: None,
            pixel_position_of_newest_cursor: None,
            last_bounds: None,
            last_line_height: None,
            focus: cx.focus_handle(),
            blink_manager,
            blink_manager_initialized: false,
            soft_wrap: SoftWrap::default(),
            preferred_line_length: 80,
        }
    }

    fn selection_for_utf16_range(&self, range: Range<usize>, cx: &App) -> Option<SelectionSet> {
        let snapshot = self.buffer.read(cx).snapshot();
        let start = snapshot
            .utf16_cu_to_byte(Utf16Offset::new(range.start))
            .ok()?;
        let end = snapshot
            .utf16_cu_to_byte(Utf16Offset::new(range.end))
            .ok()?;
        Some(SelectionSet::new(vec![Selection::new(start, end)]))
    }

    fn replace_text(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let before_selections = self.selections.clone();
        let composition = self.composition.take();
        let targets = match self.replacement_targets(composition.as_ref(), range_utf16, cx) {
            Some(targets) => targets,
            None => {
                self.composition = composition;
                return;
            }
        };
        let text = if self.mode == EditorMode::SingleLine {
            text.replace(['\r', '\n'], "")
        } else {
            text.to_owned()
        };
        let merge_with_composition = composition
            .as_ref()
            .and_then(|composition| composition.history_transaction_id)
            .is_some_and(|transaction_id| self.is_current_history_transaction(transaction_id, cx));
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = replace_selections(
                buffer,
                &targets,
                &text,
                input_metadata("输入文本", merge_with_composition),
            );
            cx.notify();
            outcome
        });
        let succeeded = outcome.is_ok();
        self.apply_edit_outcome(before_selections, outcome, cx);
        if !succeeded {
            self.composition = composition;
        }
    }

    fn replacement_targets(
        &self,
        composition: Option<&EditorComposition>,
        range_utf16: Option<Range<usize>>,
        cx: &App,
    ) -> Option<SelectionSet> {
        if let Some(composition) = composition {
            let ranges = composition
                .ranges
                .iter()
                .copied()
                .map(|range| {
                    range_utf16.clone().map_or(Some(range), |relative_range| {
                        self.relative_utf16_range(range, relative_range, cx)
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            return Some(SelectionSet::new_with_primary(
                ranges
                    .into_iter()
                    .map(|range| Selection::new(range.start(), range.end()))
                    .collect(),
                composition.primary_index,
            ));
        }
        if let Some(range) = range_utf16 {
            return self.selection_for_utf16_range(range, cx);
        }
        Some(self.selections.clone())
    }

    fn relative_utf16_range(
        &self,
        containing_range: TextRange,
        relative_range: Range<usize>,
        cx: &App,
    ) -> Option<TextRange> {
        let snapshot = self.buffer.read(cx).snapshot();
        let text = snapshot.slice_text(containing_range).ok()?;
        let text = text.as_str();
        let utf16_len = utf16_len(text);
        let start = byte_for_utf16_offset(text, relative_range.start.min(utf16_len))?;
        let end = byte_for_utf16_offset(text, relative_range.end.min(utf16_len))?;
        TextRange::new(
            ByteOffset::new(containing_range.start().get() + start),
            ByteOffset::new(containing_range.start().get() + end),
        )
        .ok()
    }

    fn is_current_history_transaction(&self, transaction_id: TransactionId, cx: &App) -> bool {
        let buffer = self.buffer.read(cx);
        buffer
            .current_history_node()
            .and_then(|node| buffer.history_node(node))
            .is_some_and(|node| node.transaction_id == transaction_id)
    }

    fn apply_edit_outcome(
        &mut self,
        before_selections: SelectionSet,
        outcome: EngineResult<EditOutcome>,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(outcome) => {
                if let Some(transaction_id) = outcome.history_transaction_id() {
                    self.selection_history.record_transaction(
                        transaction_id,
                        before_selections,
                        outcome.after_selections().clone(),
                    );
                }
                self.selections = outcome.into_after_selections();
                self.sync_display_map(cx);
                self.request_autoscroll();
                self.input_layout = None;
                self.blink_manager.update(cx, |blink, cx| {
                    blink.pause_blinking(cx);
                });
                cx.notify();
            }
            Err(error) => eprintln!("Editor 编辑事务失败：{error}"),
        }
    }

    fn move_selections(
        &mut self,
        direction: MovementDirection,
        motion: impl Into<Motion>,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        let motion = motion.into();
        let primary_index = self.selections.primary_index();
        let outcome = self
            .selections
            .as_slice()
            .iter()
            .copied()
            .map(|selection| {
                let new_head = match motion {
                    Motion::ByUnit(unit) => {
                        // 软换行模式下行首/行尾按显示行边界移动（对齐 Zed 的
                        // prev/next_row_boundary 语义），其余单位走文本边界。
                        if unit == MovementUnit::LineEdge && self.display_map.is_wrapped() {
                            let head = selection.head();
                            return match direction {
                                MovementDirection::Previous => self
                                    .display_map
                                    .beginning_of_row(head)
                                    .map_err(|error| zcv_engine::EngineError::EngineBug {
                                        location: "Editor::move_selections",
                                        detail: error.to_string(),
                                    }),
                                MovementDirection::Next => self
                                    .display_map
                                    .end_of_row(head)
                                    .map_err(|error| zcv_engine::EngineError::EngineBug {
                                        location: "Editor::move_selections",
                                        detail: error.to_string(),
                                    }),
                            }
                            .map(|new_head| {
                                if extend {
                                    selection.with_head(new_head)
                                } else {
                                    Selection::caret(new_head)
                                }
                            });
                        }
                        let buffer = self.buffer.read(cx);
                        let head = buffer.byte_to_char(selection.head())?;
                        let target = buffer.movement_boundary(head, direction, unit)?;
                        buffer.char_to_byte(target)?
                    }
                    Motion::LineStep | Motion::PageStep(_) => {
                        let row_step = match motion {
                            Motion::PageStep(row_step) => row_step,
                            _ => 1,
                        };
                        let point = self
                            .display_map
                            .offset_to_display_point(selection.head())
                            .map_err(|error| zcv_engine::EngineError::EngineBug {
                                location: "Editor::move_selections",
                                detail: error.to_string(),
                            })?;
                        let last_row = self.display_map.line_count().saturating_sub(1);
                        if direction == MovementDirection::Previous
                            && point.row() == DisplayRow::ZERO
                        {
                            return Ok(if extend {
                                selection.with_head(ByteOffset::ZERO)
                            } else {
                                Selection::caret(ByteOffset::ZERO)
                            });
                        }
                        if direction == MovementDirection::Next && point.row().get() >= last_row {
                            let new_head = self.display_map.buffer_snapshot().len_bytes();
                            return Ok(if extend {
                                selection.with_head(new_head)
                            } else {
                                Selection::caret(new_head)
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
                                point.column(),
                            ))
                            .map_err(|error| zcv_engine::EngineError::EngineBug {
                                location: "Editor::move_selections",
                                detail: error.to_string(),
                            })?
                    }
                    Motion::DocumentEdge => match direction {
                        MovementDirection::Previous => ByteOffset::ZERO,
                        MovementDirection::Next => self.buffer.read(cx).len_bytes(),
                    },
                };
                Ok(if extend {
                    selection.with_head(new_head)
                } else {
                    Selection::caret(new_head)
                })
            })
            .collect::<EngineResult<Vec<_>>>()
            .map(|selections| SelectionSet::new_with_primary(selections, primary_index));
        match outcome {
            Ok(selections) => {
                self.composition = None;
                self.selections = selections;
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

    fn delete(
        &mut self,
        direction: MovementDirection,
        unit: MovementUnit,
        description: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.composition = None;
        let before_selections = self.selections.clone();
        let targets = self.delete_targets(&before_selections, Some((direction, unit)), cx);
        let outcome = targets.and_then(|targets| {
            self.buffer.update(cx, |buffer, cx| {
                let outcome = replace_selections(buffer, &targets, "", edit_metadata(description));
                cx.notify();
                outcome
            })
        });
        self.apply_edit_outcome(before_selections, outcome, cx);
    }

    fn delete_to_line_edge(
        &mut self,
        direction: MovementDirection,
        description: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.composition = None;
        let before_selections = self.selections.clone();
        let targets = {
            let buffer = self.buffer.read(cx);
            before_selections
                .as_slice()
                .iter()
                .map(|selection| {
                    let range = selection.range();
                    let pivot = match direction {
                        MovementDirection::Previous => range.start(),
                        MovementDirection::Next => range.end(),
                    };
                    let pivot_char = buffer.byte_to_char(pivot)?;
                    let mut boundary = buffer.char_to_byte(buffer.movement_boundary(
                        pivot_char,
                        direction,
                        MovementUnit::LineEdge,
                    )?)?;
                    if selection.is_caret() && boundary == pivot {
                        boundary = buffer.char_to_byte(buffer.movement_boundary(
                            pivot_char,
                            direction,
                            MovementUnit::Grapheme,
                        )?)?;
                    }
                    Ok(match direction {
                        MovementDirection::Previous => Selection::new(boundary, range.end()),
                        MovementDirection::Next => Selection::new(range.start(), boundary),
                    })
                })
                .collect::<EngineResult<Vec<_>>>()
                .map(SelectionSet::new)
        };
        let outcome = targets.and_then(|targets| {
            self.buffer.update(cx, |buffer, cx| {
                let outcome = replace_selections(buffer, &targets, "", edit_metadata(description));
                cx.notify();
                outcome
            })
        });
        self.apply_edit_outcome(before_selections, outcome, cx);
    }

    fn delete_targets(
        &self,
        selections: &SelectionSet,
        caret_motion: Option<(MovementDirection, MovementUnit)>,
        cx: &App,
    ) -> EngineResult<SelectionSet> {
        let buffer = self.buffer.read(cx);
        let mut targets = Vec::new();
        for selection in selections.as_slice() {
            if !selection.is_caret() {
                targets.push(*selection);
                continue;
            }
            let Some((direction, unit)) = caret_motion else {
                continue;
            };
            let head_char = buffer.byte_to_char(selection.head())?;
            let boundary =
                buffer.char_to_byte(buffer.movement_boundary(head_char, direction, unit)?)?;
            if boundary != selection.head() {
                targets.push(match direction {
                    MovementDirection::Previous => Selection::new(boundary, selection.head()),
                    MovementDirection::Next => Selection::new(selection.head(), boundary),
                });
            }
        }
        if targets.is_empty() {
            Ok(selections.clone())
        } else {
            Ok(SelectionSet::new(targets))
        }
    }

    fn indent(&mut self, cx: &mut Context<Self>) {
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
            return;
        }
        let before = self.selections.normalized();
        let snapshot = self.buffer.read(cx).snapshot();
        let tab = snapshot.config().tab;
        let all_carets = before
            .as_slice()
            .iter()
            .all(|selection| selection.is_caret());
        let targets = if all_carets {
            before
                .as_slice()
                .iter()
                .map(|selection| {
                    let text: Arc<str> = if tab.insert_spaces {
                        let column = self
                            .display_map
                            .offset_to_display_point(selection.head())
                            .map_err(|error| zcv_engine::EngineError::EngineBug {
                                location: "Editor::indent",
                                detail: error.to_string(),
                            })?
                            .column()
                            .get();
                        let width = tab.indent_width();
                        Arc::from(" ".repeat(width - column % width))
                    } else {
                        Arc::from("\t")
                    };
                    Ok((*selection, text))
                })
                .collect::<EngineResult<Vec<_>>>()
        } else {
            touched_lines(&snapshot, &before).map(|lines| {
                let text: Arc<str> = if tab.insert_spaces {
                    Arc::from(" ".repeat(tab.indent_width()))
                } else {
                    Arc::from("\t")
                };
                lines
                    .into_iter()
                    .map(|line| {
                        (
                            Selection::caret(
                                snapshot
                                    .line_start_byte(line)
                                    .expect("已验证逻辑行必须有行首"),
                            ),
                            Arc::clone(&text),
                        )
                    })
                    .collect()
            })
        };
        let outcome = targets.and_then(|targets| {
            self.buffer.update(cx, |buffer, cx| {
                let outcome =
                    apply_targeted_edits(buffer, targets, &before, edit_metadata("增加缩进"));
                cx.notify();
                outcome
            })
        });
        self.apply_edit_outcome(before, outcome, cx);
    }

    fn outdent(&mut self, cx: &mut Context<Self>) {
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
            return;
        }
        let before = self.selections.clone();
        let snapshot = self.buffer.read(cx).snapshot();
        let targets = touched_lines(&snapshot, &before).and_then(|lines| {
            lines
                .into_iter()
                .filter_map(|line| match leading_indent_range(&snapshot, line) {
                    Ok(Some(selection)) => Some(Ok((selection, Arc::from("")))),
                    Ok(None) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect::<EngineResult<Vec<_>>>()
        });
        let outcome = targets.and_then(|targets| {
            self.buffer.update(cx, |buffer, cx| {
                let outcome =
                    apply_targeted_edits(buffer, targets, &before, edit_metadata("减少缩进"));
                cx.notify();
                outcome
            })
        });
        self.apply_edit_outcome(before, outcome, cx);
    }

    fn insert_newline(&mut self, cx: &mut Context<Self>) {
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
            return;
        }
        self.composition = None;
        let before = self.selections.normalized();
        let snapshot = self.buffer.read(cx).snapshot();
        let targets = before
            .as_slice()
            .iter()
            .map(|selection| {
                let offset = selection.start();
                let line = snapshot.byte_to_line(offset)?;
                let line_start = snapshot.line_start_byte(line)?;
                let prefix = snapshot.slice_byte_range(line_start, offset)?;
                let leading: String = prefix
                    .as_str()
                    .chars()
                    .take_while(|character| matches!(character, ' ' | '\t'))
                    .collect();
                let query_start = offset.get().saturating_sub(1);
                let query_end = offset
                    .get()
                    .saturating_add(1)
                    .min(snapshot.len_bytes().get());
                let should_indent = self
                    .syntax_snapshot
                    .indent_ranges(query_start..query_end, &snapshot)
                    .into_iter()
                    .any(|range| {
                        range.range.start < offset.get()
                            && offset.get() <= range.range.end
                            && range
                                .end
                                .as_ref()
                                .is_none_or(|end| offset.get() <= end.start)
                    });
                let indent = if should_indent {
                    if snapshot.config().tab.insert_spaces {
                        " ".repeat(snapshot.config().tab.indent_width())
                    } else {
                        "\t".to_owned()
                    }
                } else {
                    String::new()
                };
                Ok((*selection, Arc::from(format!("\n{leading}{indent}"))))
            })
            .collect::<EngineResult<Vec<_>>>();
        let outcome = targets.and_then(|targets| {
            self.buffer.update(cx, |buffer, cx| {
                let outcome =
                    apply_targeted_edits(buffer, targets, &before, edit_metadata("插入换行"));
                cx.notify();
                outcome
            })
        });
        self.apply_edit_outcome(before, outcome, cx);
    }

    fn selected_text(&self, cx: &App) -> Option<String> {
        let snapshot = self.buffer.read(cx).snapshot();
        let mut parts = Vec::new();
        for selection in self.selections.as_slice() {
            if selection.is_caret() {
                continue;
            }
            parts.push(
                snapshot
                    .slice_text(selection.range())
                    .ok()?
                    .as_str()
                    .to_owned(),
            );
        }
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = buffer.undo();
            cx.notify();
            outcome
        });
        match outcome {
            Ok(Some(outcome)) => {
                if let Some(selections) = self
                    .selection_history
                    .transaction(outcome.transaction_id())
                    .map(|history| history.undo().clone())
                {
                    self.selections = selections;
                }
                self.synchronize_after_history_edit(cx);
            }
            Ok(None) => {}
            Err(error) => eprintln!("Editor Undo 失败：{error}"),
        }
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = buffer.redo();
            cx.notify();
            outcome
        });
        match outcome {
            Ok(Some(outcome)) => {
                if let Some(selections) = self
                    .selection_history
                    .transaction(outcome.transaction_id())
                    .map(|history| history.redo().clone())
                {
                    self.selections = selections;
                }
                self.synchronize_after_history_edit(cx);
            }
            Ok(None) => {}
            Err(error) => eprintln!("Editor Redo 失败：{error}"),
        }
    }

    fn synchronize_after_history_edit(&mut self, cx: &mut Context<Self>) {
        self.composition = None;
        self.sync_display_map(cx);
        self.request_autoscroll();
        self.input_layout = None;
        cx.notify();
    }

    fn request_autoscroll(&mut self) {
        let head = self.selections.primary().head();
        if let Ok(point) = self.display_map.offset_to_display_point(head) {
            self.scroll_manager.request_autoscroll(point);
        }
    }

    fn sync_display_map(&mut self, cx: &App) {
        let snapshot = self.buffer.read(cx).snapshot();
        let changes = self.buffer_subscription.consume();
        self.display_map.sync(snapshot, changes);
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
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
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
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
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
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
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
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
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
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
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
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
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
        let end = self.buffer.read(cx).len_bytes();
        self.composition = None;
        self.selections = SelectionSet::new(vec![Selection::new(ByteOffset::ZERO, end)]);
        self.request_autoscroll();
        self.input_layout = None;
        cx.notify();
    }

    pub(super) fn handle_expand_selection(
        &mut self,
        _: &ExpandSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self.buffer.read(cx).snapshot();
        let expanded = self
            .selections
            .as_slice()
            .iter()
            .map(|selection| {
                let range = selection.start().get()..selection.end().get();
                self.syntax_snapshot
                    .ancestor_range(range, &snapshot)
                    .map(|range| {
                        Selection::new(ByteOffset::new(range.start), ByteOffset::new(range.end))
                    })
                    .unwrap_or(*selection)
            })
            .collect();
        self.composition = None;
        self.selections = SelectionSet::new_with_primary(expanded, self.selections.primary_index());
        self.request_autoscroll();
        self.input_layout = None;
        cx.notify();
    }

    pub(super) fn handle_backspace(
        &mut self,
        _: &Backspace,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete(
            MovementDirection::Previous,
            MovementUnit::Grapheme,
            "向后删除",
            cx,
        );
    }

    pub(super) fn handle_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.delete(
            MovementDirection::Next,
            MovementUnit::Grapheme,
            "向前删除",
            cx,
        );
    }

    pub(super) fn handle_delete_to_previous_word_start(
        &mut self,
        _: &DeleteToPreviousWordStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete(
            MovementDirection::Previous,
            MovementUnit::Word,
            "删除到前一个词",
            cx,
        );
    }

    pub(super) fn handle_delete_to_next_word_end(
        &mut self,
        _: &DeleteToNextWordEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete(
            MovementDirection::Next,
            MovementUnit::Word,
            "删除到后一个词",
            cx,
        );
    }

    pub(super) fn handle_delete_to_beginning_of_line(
        &mut self,
        _: &DeleteToBeginningOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_to_line_edge(MovementDirection::Previous, "删除到行首", cx);
    }

    pub(super) fn handle_delete_to_end_of_line(
        &mut self,
        _: &DeleteToEndOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_to_line_edge(MovementDirection::Next, "删除到行尾", cx);
    }

    pub(super) fn handle_newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_newline(cx);
    }

    pub(super) fn handle_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo(cx);
    }

    pub(super) fn handle_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.redo(cx);
    }

    pub(super) fn handle_copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text(cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn handle_cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.selected_text(cx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.composition = None;
        let before_selections = self.selections.clone();
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = replace_selections(buffer, &before_selections, "", edit_metadata("剪切"));
            cx.notify();
            outcome
        });
        self.apply_edit_outcome(before_selections, outcome, cx);
    }

    pub(super) fn handle_paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = item.text() else {
            return;
        };
        if !text.is_empty() {
            self.replace_text(None, &text, cx);
        }
    }

    pub(super) fn handle_indent(&mut self, _: &Indent, _: &mut Window, cx: &mut Context<Self>) {
        self.indent(cx);
    }

    pub(super) fn handle_outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        self.outdent(cx);
    }
}

fn touched_lines(snapshot: &Snapshot, selections: &SelectionSet) -> EngineResult<Vec<Line>> {
    let mut lines = BTreeSet::new();
    for selection in selections.as_slice() {
        let range = selection.range();
        let start = snapshot.byte_to_line(range.start())?;
        let mut end = snapshot.byte_to_line(range.end())?;
        if !range.is_empty() && end > start && snapshot.line_start_byte(end)? == range.end() {
            end = Line::new(end.get() - 1);
        }
        lines.extend((start.get()..=end.get()).map(Line::new));
    }
    Ok(lines.into_iter().collect())
}

fn leading_indent_range(snapshot: &Snapshot, line: Line) -> EngineResult<Option<Selection>> {
    let start = snapshot.line_start_byte(line)?;
    let text = snapshot.slice_line(line)?;
    let content = text.as_str();
    let end = if content.starts_with('\t') {
        start.checked_add(1)
    } else {
        let spaces = content
            .bytes()
            .take(snapshot.config().tab.indent_width())
            .take_while(|byte| *byte == b' ')
            .count();
        start.checked_add(spaces)
    };
    Ok(end
        .filter(|end| *end > start)
        .map(|end| Selection::new(start, end)))
}

fn edit_metadata(description: &'static str) -> TransactionMetadata {
    TransactionMetadata::new(TransactionSource::Programmatic).with_description(description)
}

fn input_metadata(description: &'static str, merge_with_previous: bool) -> TransactionMetadata {
    let metadata = edit_metadata(description);
    if merge_with_previous {
        metadata.with_merge_policy(TransactionMergePolicy::MergeWithPrevious)
    } else {
        metadata
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
            cx.on_focus(&self.focus, window, |editor, _window, cx| {
                editor.blink_manager.update(cx, BlinkManager::enable);
            })
            .detach();
            cx.on_blur(&self.focus, window, |editor, _window, cx| {
                editor.blink_manager.update(cx, BlinkManager::disable);
            })
            .detach();
            self.blink_manager_initialized = true;
        }

        // 同步当前焦点状态——弥补焦点先于首次 render 到达的时序缺口。
        if self.focus.is_focused(window) {
            self.blink_manager.update(cx, |b, cx| b.enable(cx));
        } else {
            self.blink_manager.update(cx, |b, cx| b.disable(cx));
        }

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
                .text_color(color::current().text),
            cx,
        )
        .child(EditorElement::new(cx.entity()))
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let presentation = self.presentation();
        actual_range.replace(range_utf16.clone());
        presentation.text_for_utf16_range(range_utf16)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let snapshot = self.display_map.buffer_snapshot();
        let selection = *self.selections.primary();
        Some(UTF16Selection {
            range: snapshot.byte_to_utf16_cu(selection.start()).ok()?.get()
                ..snapshot.byte_to_utf16_cu(selection.end()).ok()?.get(),
            reversed: selection.is_reversed(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.presentation().marked_utf16_range()
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.composition = None;
        self.input_layout = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text(range_utf16, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_composition = self.composition.take();
        let Some(targets) =
            self.replacement_targets(previous_composition.as_ref(), range_utf16, cx)
        else {
            self.composition = previous_composition;
            return;
        };
        let text = if self.mode == EditorMode::SingleLine {
            new_text.replace(['\r', '\n'], "")
        } else {
            new_text.to_owned()
        };
        let previous_history_transaction = previous_composition
            .as_ref()
            .and_then(|composition| composition.history_transaction_id);
        let merge_with_composition = previous_history_transaction
            .is_some_and(|transaction_id| self.is_current_history_transaction(transaction_id, cx));
        let before_selections = self.selections.clone();
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = replace_selections(
                buffer,
                &targets,
                &text,
                input_metadata("输入法组合", merge_with_composition),
            );
            cx.notify();
            outcome
        });
        let history_transaction_id = outcome
            .as_ref()
            .ok()
            .and_then(EditOutcome::history_transaction_id)
            .or(previous_history_transaction);
        if outcome.is_err() {
            self.composition = previous_composition;
            self.apply_edit_outcome(before_selections, outcome, cx);
            return;
        }
        self.apply_edit_outcome(before_selections.clone(), outcome, cx);
        if text.is_empty() {
            self.composition = None;
            return;
        }

        let inserted_selections = self.selections.clone();
        let marked_ranges = inserted_selections
            .as_slice()
            .iter()
            .map(|selection| {
                let end = selection.head();
                let start = ByteOffset::new(end.get().saturating_sub(text.len()));
                TextRange::new(start, end).expect("替换后的选区必须能够还原出 marked text 范围")
            })
            .collect::<Vec<_>>();
        let text_utf16_len = utf16_len(&text);
        let selected_range_utf16 =
            new_selected_range_utf16.unwrap_or(text_utf16_len..text_utf16_len);
        let selected_start =
            byte_for_utf16_offset(&text, selected_range_utf16.start.min(text_utf16_len))
                .unwrap_or(text.len());
        let selected_end =
            byte_for_utf16_offset(&text, selected_range_utf16.end.min(text_utf16_len))
                .unwrap_or(text.len());
        self.selections = SelectionSet::new_with_primary(
            marked_ranges
                .iter()
                .map(|marked_range| {
                    Selection::new(
                        ByteOffset::new(marked_range.start().get() + selected_start),
                        ByteOffset::new(marked_range.start().get() + selected_end),
                    )
                })
                .collect(),
            inserted_selections.primary_index(),
        );
        if let Some(transaction_id) = history_transaction_id {
            self.selection_history.record_transaction(
                transaction_id,
                before_selections,
                self.selections.clone(),
            );
        }
        self.composition = Some(EditorComposition {
            ranges: marked_ranges.into(),
            primary_index: inserted_selections.primary_index(),
            history_transaction_id,
        });
        self.request_autoscroll();
        self.input_layout = None;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let cursor = self.pixel_position_of_newest_cursor?;
        let bounds = self.last_bounds?;
        Some(Bounds::new(
            point(bounds.origin.x + cursor.x, bounds.origin.y + cursor.y),
            size(px(2.), self.last_line_height?),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.input_layout.as_ref()?.utf16_index_for_point(point)
    }
}

#[cfg(test)]
#[path = "test/selection_edit_tests.rs"]
mod selection_edit_tests;

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn byte_for_utf16_offset(text: &str, target: usize) -> Option<usize> {
    let mut utf16_offset = 0;
    for (byte_offset, character) in text.char_indices() {
        if utf16_offset == target {
            return Some(byte_offset);
        }
        utf16_offset += character.len_utf16();
        if utf16_offset > target {
            return None;
        }
    }
    (utf16_offset == target).then_some(text.len())
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, Bounds, ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};
    use zcv_engine::{BufferConfig, ByteOffset, SelectionSet, TransactionId};

    use super::*;
    use crate::display_map::{DisplayPoint, DisplayRow};

    fn test_buffer(cx: &mut TestAppContext, text: impl Into<String>) -> Entity<LanguageBuffer> {
        let buffer =
            Buffer::scratch(text.into(), BufferConfig::default()).expect("测试 Buffer 应能创建");
        let buffer = cx.new(|_| buffer);
        cx.new(|cx| LanguageBuffer::new(buffer, None, cx))
    }

    fn engine_buffer(buffer: &Entity<LanguageBuffer>, cx: &TestAppContext) -> Entity<Buffer> {
        cx.read_entity(buffer, |buffer, _| buffer.buffer())
    }

    fn buffer_text(buffer: &Entity<LanguageBuffer>, cx: &TestAppContext) -> String {
        let buffer = engine_buffer(buffer, cx);
        cx.read_entity(&buffer, |buffer, _| {
            buffer
                .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
                .expect("完整测试 Buffer 应可读取")
                .as_str()
                .to_owned()
        })
    }

    #[gpui::test]
    fn editors_share_buffer_but_keep_view_state_independent(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "abc");
        let first = cx.new(|cx| Editor::for_buffer(buffer.clone(), cx));
        let second = cx.new(|cx| Editor::for_buffer(buffer.clone(), cx));

        cx.update_entity(&first, |editor, cx| {
            editor.selections = SelectionSet::caret(ByteOffset::new(1));
            editor
                .scroll_manager
                .update_viewport(1, px(100.0), px(40.0), px(200.0), px(20.0));
            editor.scroll_manager.scroll_by(point(px(-4.0), px(0.0)));
            editor.selection_history.record_transaction(
                TransactionId::new(1),
                SelectionSet::caret(ByteOffset::ZERO),
                editor.selections.clone(),
            );
            editor.buffer.update(cx, |buffer, cx| {
                buffer
                    .insert(ByteOffset::new(3), "d")
                    .expect("共享 Buffer 编辑应成功");
                cx.notify();
            });
        });

        cx.read_entity(&second, |editor, cx| {
            assert_eq!(editor.mode, EditorMode::Full);
            assert_eq!(editor.language_buffer, buffer);
            assert_eq!(editor.buffer.read(cx).len_bytes(), ByteOffset::new(4));
            assert_eq!(editor.render_snapshot().len_bytes(), ByteOffset::new(4));
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::ZERO));
            assert_eq!(editor.scroll_manager.anchor(), DisplayPoint::ZERO);
            assert_eq!(editor.scroll_manager.offset(), point(px(0.0), px(0.0)));
            assert!(
                editor
                    .selection_history
                    .transaction(TransactionId::new(1))
                    .is_none()
            );
        });

        cx.read_entity(&first, |editor, _| {
            assert_eq!(editor.scroll_manager.offset().x, px(4.0));
            let history = editor
                .selection_history
                .transaction(TransactionId::new(1))
                .expect("第一个 Editor 应保存自己的选区历史");
            assert_eq!(history.undo(), &SelectionSet::caret(ByteOffset::ZERO));
            assert_eq!(history.redo(), &SelectionSet::caret(ByteOffset::new(1)));
        });
    }

    #[gpui::test]
    fn constructors_create_expected_modes_and_independent_scratch_buffers(cx: &mut TestAppContext) {
        let single_line = cx.new(Editor::single_line);
        let auto_height = cx.new(|cx| Editor::auto_height(2, Some(6), cx));

        let single_buffer = cx.read_entity(&single_line, |editor, cx| {
            assert_eq!(editor.mode, EditorMode::SingleLine);
            assert_eq!(editor.selections, SelectionSet::default());
            assert_eq!(
                editor.display_map.version(),
                editor.buffer.read(cx).version()
            );
            let _focus = editor.focus_handle();
            editor.buffer.clone()
        });
        let auto_height_buffer = cx.read_entity(&auto_height, |editor, _| {
            assert_eq!(
                editor.mode,
                EditorMode::AutoHeight {
                    min_lines: 2,
                    max_lines: Some(6),
                }
            );
            editor.buffer.clone()
        });

        assert_ne!(single_buffer, auto_height_buffer);
    }

    #[gpui::test]
    fn editor_element_renders_multiline_unicode_text(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "a你\n😀b");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.run_until_parked();
        cx.simulate_click(point(px(1000.), px(12.)), gpui::Modifiers::default());
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.render_snapshot().line_count(), 2);
            assert_eq!(editor.selections.primary().head(), ByteOffset::new(4));
        });
    }

    #[gpui::test]
    fn clicking_the_gutter_selects_a_logical_line(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "first\nsecond\nthird");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.run_until_parked();
        cx.simulate_click(point(px(4.), px(32.)), gpui::Modifiers::default());

        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections,
                SelectionSet::new(vec![Selection::new(
                    ByteOffset::new(6),
                    ByteOffset::new(13)
                )])
            );
        });

        cx.simulate_click(
            point(px(4.), px(58.)),
            gpui::Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections,
                SelectionSet::new(vec![Selection::new(
                    ByteOffset::new(6),
                    ByteOffset::new(18)
                )])
            );
        });
    }

    #[gpui::test]
    fn committed_input_uses_element_input_handler_and_preserves_unicode(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.simulate_click(point(px(4.), px(12.)), gpui::Modifiers::default());
        cx.simulate_input("中😀e\u{301}");

        assert_eq!(buffer_text(&buffer, cx), "中😀e\u{301}");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections.primary().head(),
                ByteOffset::new("中😀e\u{301}".len())
            );
            assert!(editor.composition.is_none());
        });
    }

    #[gpui::test]
    fn editor_actions_move_extend_delete_and_restore_unicode_selection(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "a😀b");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));
        cx.dispatch_action(MoveRight);
        cx.dispatch_action(SelectRight);
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections,
                SelectionSet::new(vec![Selection::new(ByteOffset::new(1), ByteOffset::new(5))])
            );
        });

        cx.dispatch_action(Backspace);
        assert_eq!(buffer_text(&buffer, cx), "ab");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::new(1)));
        });

        cx.dispatch_action(Undo);
        assert_eq!(buffer_text(&buffer, cx), "a😀b");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections,
                SelectionSet::new(vec![Selection::new(ByteOffset::new(1), ByteOffset::new(5))])
            );
        });

        cx.dispatch_action(Redo);
        assert_eq!(buffer_text(&buffer, cx), "ab");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::new(1)));
        });
    }

    #[gpui::test]
    fn expand_selection_uses_tree_sitter_ancestors(cx: &mut TestAppContext) {
        let source = "fn main() { let value = 1; }\n";
        let raw_buffer = cx.new(|_| {
            Buffer::scratch(source.to_owned(), BufferConfig::default())
                .expect("Rust 测试 Buffer 应能创建")
        });
        let language_buffer = cx.new({
            let raw_buffer = raw_buffer.clone();
            move |cx| LanguageBuffer::new(raw_buffer, Some(PathBuf::from("main.rs")), cx)
        });
        cx.run_until_parked();
        let (editor, cx) = cx.add_window_view({
            let language_buffer = language_buffer.clone();
            move |_, cx| Editor::for_buffer(language_buffer, cx)
        });
        cx.run_until_parked();
        let value = source.find("value").unwrap();
        cx.update_entity(&editor, |editor, _| {
            editor.selections = SelectionSet::caret(ByteOffset::new(value));
        });
        cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

        cx.dispatch_action(ExpandSelection);
        cx.read_entity(&editor, |editor, _| {
            let selection = editor.selections.primary().range();
            assert_eq!(
                &source[selection.start().get()..selection.end().get()],
                "value"
            );
        });
        cx.dispatch_action(ExpandSelection);
        cx.read_entity(&editor, |editor, _| {
            assert!(editor.selections.primary().range().len() > "value".len());
        });
    }

    #[gpui::test]
    fn matching_brackets_come_from_tree_sitter_query(cx: &mut TestAppContext) {
        let source = "fn main() { call(); }\n";
        let raw_buffer = cx.new(|_| {
            Buffer::scratch(source.to_owned(), BufferConfig::default())
                .expect("Rust 测试 Buffer 应能创建")
        });
        let language_buffer = cx.new({
            let raw_buffer = raw_buffer.clone();
            move |cx| LanguageBuffer::new(raw_buffer, Some(PathBuf::from("main.rs")), cx)
        });
        let editor = cx.new({
            let language_buffer = language_buffer.clone();
            move |cx| Editor::for_buffer(language_buffer, cx)
        });
        cx.run_until_parked();
        let open = source.find("()").unwrap();
        cx.update_entity(&editor, |editor, _| {
            editor.selections = SelectionSet::caret(ByteOffset::new(open + 1));
        });

        cx.read_entity(&editor, |editor, _| {
            let pair = editor
                .matching_bracket_pair()
                .expect("光标旁的括号应由 tree-sitter query 匹配");
            assert_eq!(&source[pair.open], "(");
            assert_eq!(&source[pair.close], ")");
        });
    }

    #[gpui::test]
    fn word_and_line_delete_actions_follow_editor_boundaries(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "alpha beta gamma");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });
        cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

        cx.update_entity(&editor, |editor, _| {
            editor.selections = SelectionSet::caret(ByteOffset::new(10));
        });
        cx.dispatch_action(DeleteToPreviousWordStart);
        assert_eq!(buffer_text(&buffer, cx), "alpha  gamma");

        cx.update_entity(&editor, |editor, cx| {
            editor.set_text("alpha beta gamma", cx);
            editor.selections = SelectionSet::caret(ByteOffset::new(6));
        });
        cx.dispatch_action(DeleteToNextWordEnd);
        assert_eq!(buffer_text(&buffer, cx), "alpha  gamma");

        cx.update_entity(&editor, |editor, cx| {
            editor.set_text("one two three four", cx);
            editor.selections = SelectionSet::new(vec![Selection::new(
                ByteOffset::new(4),
                ByteOffset::new(13),
            )]);
        });
        cx.dispatch_action(DeleteToBeginningOfLine);
        assert_eq!(buffer_text(&buffer, cx), " four");

        cx.update_entity(&editor, |editor, cx| {
            editor.set_text("one two three four", cx);
            editor.selections = SelectionSet::new(vec![Selection::new(
                ByteOffset::new(4),
                ByteOffset::new(13),
            )]);
        });
        cx.dispatch_action(DeleteToEndOfLine);
        assert_eq!(buffer_text(&buffer, cx), "one ");

        cx.update_entity(&editor, |editor, cx| {
            editor.set_text("one\ntwo", cx);
            editor.selections = SelectionSet::caret(ByteOffset::new(4));
        });
        cx.dispatch_action(DeleteToBeginningOfLine);
        assert_eq!(buffer_text(&buffer, cx), "onetwo");
    }

    #[gpui::test]
    fn document_boundary_actions_move_and_extend_selection(cx: &mut TestAppContext) {
        let text = "ab\n中😀z";
        let buffer = test_buffer(cx, text);
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });
        let end = ByteOffset::new(text.len());

        cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));
        cx.dispatch_action(MoveToEnd);
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(end));
        });

        cx.dispatch_action(MoveToBeginning);
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::ZERO));
        });

        let anchor = ByteOffset::new(2);
        cx.update_entity(&editor, |editor, _| {
            editor.selections = SelectionSet::caret(anchor);
        });
        cx.dispatch_action(SelectToEnd);
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections,
                SelectionSet::new(vec![Selection::new(anchor, end)])
            );
        });

        cx.update_entity(&editor, |editor, _| {
            editor.selections = SelectionSet::caret(anchor);
        });
        cx.dispatch_action(SelectToBeginning);
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections,
                SelectionSet::new(vec![Selection::new(anchor, ByteOffset::ZERO)])
            );
        });
    }

    #[gpui::test]
    fn page_actions_move_selection_and_viewport_together(cx: &mut TestAppContext) {
        let text = (0..40).map(|row| format!("{row}\n")).collect::<String>();
        let buffer = test_buffer(cx, text);
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.simulate_resize(size(px(100.), px(100.)));
        cx.run_until_parked();
        cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

        let page_rows = cx.read_entity(&editor, |editor, _| {
            editor
                .scroll_manager
                .page_row_count()
                .expect("完成布局后应有可见页行数")
        });
        assert!(page_rows > 0);

        cx.dispatch_action(MovePageDown);
        cx.run_until_parked();
        cx.read_entity(&editor, |editor, _| {
            let caret_row = editor
                .render_snapshot()
                .byte_to_position(editor.selections.primary().head())
                .expect("翻页后的光标应有效")
                .line()
                .get();
            assert_eq!(caret_row, page_rows);
            assert_eq!(
                editor.scroll_manager.anchor().row(),
                DisplayRow::new(page_rows)
            );
        });

        let raw_buffer = engine_buffer(&buffer, cx);
        let snapshot = cx.read_entity(&raw_buffer, |buffer, _| buffer.snapshot());
        let first_page = snapshot
            .line_start_byte(Line::new(page_rows))
            .expect("第一页目标行应存在");
        let second_page = snapshot
            .line_start_byte(Line::new(page_rows * 2))
            .expect("第二页目标行应存在");

        cx.dispatch_action(SelectPageDown);
        cx.run_until_parked();
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections,
                SelectionSet::new(vec![Selection::new(first_page, second_page)])
            );
            assert_eq!(
                editor.scroll_manager.anchor().row(),
                DisplayRow::new(page_rows * 2)
            );
        });

        cx.dispatch_action(MovePageUp);
        cx.run_until_parked();
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(first_page));
            assert_eq!(
                editor.scroll_manager.anchor().row(),
                DisplayRow::new(page_rows)
            );
        });

        cx.dispatch_action(SelectPageUp);
        cx.run_until_parked();
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections,
                SelectionSet::new(vec![Selection::new(first_page, ByteOffset::ZERO)])
            );
            assert_eq!(editor.scroll_manager.anchor().row(), DisplayRow::ZERO);
        });
    }

    #[gpui::test]
    fn clipboard_actions_edit_selected_text_through_transactions(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "hello");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));
        cx.update_entity(&editor, |editor, _| {
            editor.selections =
                SelectionSet::new(vec![Selection::new(ByteOffset::new(1), ByteOffset::new(4))]);
        });
        cx.dispatch_action(Copy);
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("ell".to_owned())
            );
        });

        cx.dispatch_action(Cut);
        assert_eq!(buffer_text(&buffer, cx), "ho");
        cx.dispatch_action(Undo);
        assert_eq!(buffer_text(&buffer, cx), "hello");

        cx.update_entity(&editor, |editor, _| {
            editor.selections = SelectionSet::caret(ByteOffset::new(5));
        });
        cx.dispatch_action(Paste);
        assert_eq!(buffer_text(&buffer, cx), "helloell");
        let raw_buffer = engine_buffer(&buffer, cx);
        assert!(cx.read_entity(&raw_buffer, |buffer, _| buffer.can_undo()));
    }

    #[gpui::test]
    fn moving_caret_beyond_viewport_scrolls_it_back_into_view(cx: &mut TestAppContext) {
        let text = (0..120)
            .map(|row| format!("line {row}\n"))
            .collect::<String>();
        let buffer = test_buffer(cx, text);
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));
        for _ in 0..80 {
            cx.dispatch_action(MoveDown);
        }
        cx.run_until_parked();

        cx.read_entity(&editor, |editor, _| {
            let caret = editor.selections.primary().head();
            let caret_row = editor
                .render_snapshot()
                .byte_to_position(caret)
                .expect("caret 应保持有效")
                .line()
                .get();
            assert_eq!(caret_row, 80);
            assert!(editor.scroll_manager.anchor().row().get() > 0);
            assert!(editor.scroll_manager.anchor().row().get() <= caret_row);
        });
    }

    #[gpui::test]
    fn wheel_input_updates_editor_scroll_state(cx: &mut TestAppContext) {
        let text = (0..120)
            .map(|row| format!("line {row}\n"))
            .collect::<String>();
        let buffer = test_buffer(cx, text);
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.run_until_parked();
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(4.), px(4.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
            ..Default::default()
        });

        cx.read_entity(&editor, |editor, _| {
            assert!(
                editor.scroll_manager.anchor().row() > DisplayRow::ZERO
                    || editor.scroll_manager.offset().y > px(0.)
            );
        });
    }

    #[gpui::test]
    fn horizontal_scroll_stops_at_content_edge_and_caret_autoscrolls(cx: &mut TestAppContext) {
        let text = "修改 zcv 模块时，请先阅读 zcv/docs/下的所有文档规范。同时查阅**[zed编辑器](https://github.com/zed-industries/zed)**的源码，看看zed是如何实现的，参考zed的实现方式，甚至是直接照搬zed的实现方式。".repeat(4);
        let buffer = test_buffer(cx, text.clone());
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.run_until_parked();
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(4.), px(4.)),
            delta: ScrollDelta::Pixels(point(px(-100_000.), px(0.))),
            ..Default::default()
        });
        let maximum = cx.read_entity(&editor, |editor, _| editor.scroll_manager.offset().x);
        assert!(maximum > px(0.));

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(4.), px(4.)),
            delta: ScrollDelta::Pixels(point(px(-100_000.), px(0.))),
            ..Default::default()
        });
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.scroll_manager.offset().x, maximum);
        });

        cx.update_entity(&editor, |editor, cx| {
            editor.scroll_manager.scroll_by(point(px(100_000.), px(0.)));
            editor.set_caret(ByteOffset::new(text.len()));
            cx.notify();
        });
        cx.run_until_parked();
        cx.read_entity(&editor, |editor, _| {
            let scroll_left = editor.scroll_manager.offset().x;
            assert!(scroll_left > px(0.));
            assert!(scroll_left <= maximum);
            let cursor = editor
                .pixel_position_of_newest_cursor
                .expect("行尾光标应有布局位置");
            let bounds = editor.last_bounds.expect("Editor 应保存最近布局范围");
            assert!(cursor.x + px(2.) <= bounds.size.width);
        });
    }

    #[gpui::test]
    fn word_line_and_vertical_movement_use_engine_boundaries(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "alpha 你好\nxy");
        let editor = cx.new({
            let buffer = buffer.clone();
            move |cx| {
                let mut editor = Editor::for_buffer(buffer, cx);
                editor.selections = SelectionSet::caret(ByteOffset::new("alpha 你好".len()));
                editor
            }
        });

        cx.update_entity(&editor, |editor, cx| {
            editor.move_selections(MovementDirection::Previous, MovementUnit::Word, false, cx);
            assert_eq!(editor.selections.primary().head(), ByteOffset::new(6));

            editor.move_selections(MovementDirection::Next, MovementUnit::LineEdge, false, cx);
            assert_eq!(
                editor.selections.primary().head(),
                ByteOffset::new("alpha 你好".len())
            );

            editor.move_selections(MovementDirection::Next, Motion::LineStep, false, cx);
            assert_eq!(
                editor.selections.primary().head(),
                ByteOffset::new("alpha 你好\nxy".len())
            );
        });
    }

    #[gpui::test]
    fn newline_is_a_transaction_and_undo_restores_selection(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "ab");
        let editor = cx.new({
            let buffer = buffer.clone();
            move |cx| {
                let mut editor = Editor::for_buffer(buffer, cx);
                editor.selections = SelectionSet::caret(ByteOffset::new(1));
                editor
            }
        });

        cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
        assert_eq!(buffer_text(&buffer, cx), "a\nb");
        let raw_buffer = engine_buffer(&buffer, cx);
        assert!(cx.read_entity(&raw_buffer, |buffer, _| buffer.can_undo()));
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::new(2)));
        });

        cx.update_entity(&editor, |editor, cx| editor.undo(cx));
        assert_eq!(buffer_text(&buffer, cx), "ab");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::new(1)));
        });
    }

    #[gpui::test]
    fn newline_uses_tree_sitter_indent_query(cx: &mut TestAppContext) {
        let source = "fn main() {}\n";
        let caret = source.find('{').unwrap() + 1;
        let raw_buffer = cx.new(|_| {
            Buffer::scratch(source.to_owned(), BufferConfig::default())
                .expect("Rust 测试 Buffer 应能创建")
        });
        let language_buffer = cx.new({
            let raw_buffer = raw_buffer.clone();
            move |cx| LanguageBuffer::new(raw_buffer, Some(PathBuf::from("main.rs")), cx)
        });
        let editor = cx.new({
            let language_buffer = language_buffer.clone();
            move |cx| {
                let mut editor = Editor::for_buffer(language_buffer, cx);
                editor.selections = SelectionSet::caret(ByteOffset::new(caret));
                editor
            }
        });
        cx.run_until_parked();

        cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
        assert_eq!(buffer_text(&language_buffer, cx), "fn main() {\n    }\n");
    }

    #[gpui::test]
    fn marked_text_updates_buffer_and_unmark_finishes_composition(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "ab");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| {
                let mut editor = Editor::for_buffer(buffer, cx);
                editor.selections = SelectionSet::caret(ByteOffset::new(1));
                editor
            }
        });

        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                editor.replace_and_mark_text_in_range(None, "中文😀", Some(2..2), window, cx);
            });
        });
        cx.refresh().expect("测试窗口应可刷新");
        cx.run_until_parked();

        assert_eq!(buffer_text(&buffer, cx), "a中文😀b");
        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                let marked = editor
                    .marked_text_range(window, cx)
                    .expect("应存在 marked range");
                let selected = editor
                    .selected_text_range(false, window, cx)
                    .expect("应存在 composition 相对选区");
                assert_eq!(marked, 1..5);
                assert_eq!(selected.range, 3..3);
                assert!(
                    editor
                        .bounds_for_range(marked.end..marked.end, Bounds::default(), window, cx)
                        .is_some()
                );
                editor.unmark_text(window, cx);
            });
        });

        assert_eq!(buffer_text(&buffer, cx), "a中文😀b");
        cx.read_entity(&editor, |editor, _| {
            assert!(editor.composition.is_none());
            assert_eq!(editor.selections.primary().head(), ByteOffset::new(7));
        });
    }

    #[gpui::test]
    fn ime_candidate_updates_merge_into_one_undo_step(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "ab");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| {
                let mut editor = Editor::for_buffer(buffer, cx);
                editor.selections = SelectionSet::caret(ByteOffset::new(1));
                editor
            }
        });

        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                editor.replace_and_mark_text_in_range(None, "z", None, window, cx);
                editor.replace_and_mark_text_in_range(None, "zh", None, window, cx);
                editor.replace_and_mark_text_in_range(None, "中", None, window, cx);
                editor.unmark_text(window, cx);
            });
        });
        assert_eq!(buffer_text(&buffer, cx), "a中b");

        cx.update_entity(&editor, |editor, cx| editor.undo(cx));
        assert_eq!(buffer_text(&buffer, cx), "ab");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::new(1)));
        });

        cx.update_entity(&editor, |editor, cx| editor.redo(cx));
        assert_eq!(buffer_text(&buffer, cx), "a中b");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::new(4)));
        });
    }

    #[gpui::test]
    fn ime_updates_every_cursor_and_tracks_the_primary_marked_range(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "ab cd");
        let initial_selections = SelectionSet::new_with_primary(
            vec![
                Selection::caret(ByteOffset::new(1)),
                Selection::caret(ByteOffset::new(4)),
            ],
            1,
        );
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            let initial_selections = initial_selections.clone();
            move |_, cx| {
                let mut editor = Editor::for_buffer(buffer, cx);
                editor.selections = initial_selections;
                editor
            }
        });

        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                editor.replace_and_mark_text_in_range(None, "中", None, window, cx);
                assert_eq!(editor.composition.as_ref().unwrap().ranges.len(), 2);
                assert_eq!(editor.marked_text_range(window, cx), Some(5..6));
                editor.replace_and_mark_text_in_range(None, "文", None, window, cx);
                editor.unmark_text(window, cx);
            });
        });

        assert_eq!(buffer_text(&buffer, cx), "a文b c文d");
        cx.update_entity(&editor, |editor, cx| editor.undo(cx));
        assert_eq!(buffer_text(&buffer, cx), "ab cd");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, initial_selections);
        });
    }

    #[gpui::test]
    fn ime_candidate_remains_in_the_syntax_highlight_pipeline(cx: &mut TestAppContext) {
        let source = "fn main() { let value = \"\"; }";
        let insertion = source.find("\"\"").unwrap() + 1;
        let raw_buffer = Buffer::scratch(source.to_owned(), BufferConfig::default())
            .expect("Rust 测试 Buffer 应能创建");
        let raw_buffer = cx.new(|_| raw_buffer);
        let language_buffer = cx.new({
            let raw_buffer = raw_buffer.clone();
            move |cx| LanguageBuffer::new(raw_buffer, Some(PathBuf::from("main.rs")), cx)
        });
        let (editor, cx) = cx.add_window_view({
            let language_buffer = language_buffer.clone();
            move |_, cx| {
                let mut editor = Editor::for_buffer(language_buffer, cx);
                editor.selections = SelectionSet::caret(ByteOffset::new(insertion));
                editor
            }
        });
        cx.run_until_parked();

        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                editor.replace_and_mark_text_in_range(None, "中文", None, window, cx);
            });
        });
        cx.run_until_parked();

        cx.read_entity(&editor, |editor, cx| {
            let snapshot = editor.buffer.read(cx).snapshot();
            let composition = editor.composition.as_ref().unwrap();
            let marked = composition.ranges[composition.primary_index];
            let highlights = editor
                .syntax_snapshot
                .highlights(0..snapshot.len_bytes().get(), &snapshot);
            assert!(highlights.iter().any(|highlight| {
                highlight.capture.as_ref() == "string"
                    && highlight.range.start <= marked.start().get()
                    && highlight.range.end >= marked.end().get()
            }));
        });
    }

    #[gpui::test]
    fn ime_relative_utf16_range_replaces_the_marked_subrange(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                editor.replace_and_mark_text_in_range(None, "a😀b", None, window, cx);
                editor.replace_and_mark_text_in_range(Some(1..3), "中", None, window, cx);
            });
        });

        assert_eq!(buffer_text(&buffer, cx), "a中b");
        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                assert_eq!(editor.marked_text_range(window, cx), Some(1..2));
                editor.unmark_text(window, cx);
            });
        });
        cx.update_entity(&editor, |editor, cx| editor.undo(cx));
        assert_eq!(buffer_text(&buffer, cx), "");
    }

    #[gpui::test]
    fn marked_text_can_cancel_and_committed_range_uses_utf16_offsets(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "a😀b");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                editor.replace_and_mark_text_in_range(None, "候选", None, window, cx);
                editor.replace_text_in_range(None, "", window, cx);
                assert!(editor.composition.is_none());
                editor.replace_text_in_range(Some(1..3), "你", window, cx);
            });
        });

        assert_eq!(buffer_text(&buffer, cx), "a你b");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections.primary().head(), ByteOffset::new(4));
        });
    }

    #[gpui::test]
    fn ime_candidate_bounds_survive_composition_and_scroll_layout_invalidation(
        cx: &mut TestAppContext,
    ) {
        let text = (0..40)
            .map(|row| format!("line {row}\n"))
            .collect::<String>();
        let buffer = test_buffer(cx, text);
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });
        let element_bounds = Bounds::new(point(px(100.), px(200.)), size(px(500.), px(300.)));
        let caret_bounds = Bounds::new(point(px(124.), px(260.)), size(px(2.), px(20.)));

        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                editor.set_ime_caret_geometry(element_bounds, Some(caret_bounds));
                editor.replace_and_mark_text_in_range(None, "中文", Some(2..2), window, cx);
                assert!(editor.input_layout.is_none());
                assert_eq!(
                    editor.bounds_for_range(2..2, element_bounds, window, cx),
                    Some(caret_bounds)
                );

                editor.prepare_scroll_viewport(size(px(100.), px(100.)), px(200.), px(20.));
                assert!(editor.scroll_by(point(px(0.), px(-60.)), cx));
                assert_eq!(
                    editor.bounds_for_range(2..2, element_bounds, window, cx),
                    Some(caret_bounds)
                );
            });
        });
    }

    #[gpui::test]
    fn soft_wrap_renders_continuation_rows_and_click_hits_fragment(cx: &mut TestAppContext) {
        // 超长行（超出测试窗口宽度）在 editor-width 模式下拆成多个显示行。
        let buffer = test_buffer(cx, "    aaaa bbbb cccc dddd eeee ".repeat(10));
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });
        editor.update(cx, |editor, cx| {
            editor.set_soft_wrap(SoftWrap::EditorWidth, 80, cx);
        });
        cx.run_until_parked();

        let (line_count, continuation_offset) = cx.read_entity(&editor, |editor, _| {
            let line_count = editor.display_map.line_count();
            assert!(line_count > 1, "宽行应拆成多个显示行");
            let continuation = editor
                .display_map
                .display_point_to_offset(DisplayPoint::new(
                    DisplayRow::new(1),
                    zcv_engine::DisplayColumn::ZERO,
                ))
                .expect("续行行首应可映射");
            (line_count, continuation)
        });

        // 点击第二个显示行（行高约 26px），光标应落在续行片段起点。
        // x 越过 gutter（约 60px）进入文本区，落在第二个显示行内。
        cx.simulate_click(point(px(80.), px(30.)), gpui::Modifiers::default());
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections.primary().head(),
                continuation_offset,
                "点击续行应把光标放到片段起点"
            );
        });
        assert!(line_count > 0);
    }
}
