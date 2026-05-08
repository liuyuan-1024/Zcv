//! M11 GPUI testbed：Viewport Slicing、MetadataLayer、TrackedRange、PositionMap、DeltaEvent 与 M10 既有编辑体验。
//!
//! 这个 example 是“人类体感 / UI 桥接”验证入口，不替代 `tests/m11_viewport_slicing.rs`。
//! 它继承 M10 的输入、移动、多光标、Undo / Redo、composition、reload、保存边界、
//! DeltaEvent、PositionMap、TrackedRange 与 MetadataLayer 体感，并叠加 Viewport 可见行、
//! 长行截断、大文本滚动和 Snapshot 只读切片观察。

use gpui::{
    App, Application, Bounds, Context, FocusHandle, Focusable, IntoElement, KeyBinding,
    KeyDownEvent, Render, StatefulInteractiveElement, Window, WindowBounds, WindowOptions, actions,
    black, div, prelude::*, px, rgb, size, white,
};
use zom_engine::{
    Affinity, Buffer, BufferConfig, BufferKind, CharOffset, CompositionSelection, DisplayColumn,
    EngineResult, Line, LineRange, MappingResult, MetadataLayer, MetadataLayerKind, MetadataLayers,
    MetadataLineWindow, MetadataRangeSpec, MetadataRangeUpdate, MovementDirection, MovementUnit,
    Position, Selection, SelectionSet, Stickiness, TextRange, TrackedRange, TrackedRangeUpdate,
    TrackedRangeUpdatePolicy, Viewport,
};

actions!(
    m11_testbed,
    [
        MoveLeft,
        MoveRight,
        SelectLeft,
        SelectRight,
        MoveWordLeft,
        MoveWordRight,
        SelectWordLeft,
        SelectWordRight,
        MoveIdentifierLeft,
        MoveIdentifierRight,
        SelectIdentifierLeft,
        SelectIdentifierRight,
        MoveSubwordLeft,
        MoveSubwordRight,
        SelectSubwordLeft,
        SelectSubwordRight,
        MoveSymbolLeft,
        MoveSymbolRight,
        SelectSymbolLeft,
        SelectSymbolRight,
        InsertSpace,
        InsertTab,
        InsertNewline,
        Backspace,
        Delete,
        Undo,
        Redo,
        Save,
        Reset,
        DemoMultiCursor,
        CollapseToPrimary,
        SelectAll,
        UseGraphemeMode,
        UseWordMode,
        UseIdentifierMode,
        UseSubwordMode,
        UseSymbolMode,
        MoveActiveUnitLeft,
        MoveActiveUnitRight,
        SelectActiveUnitLeft,
        SelectActiveUnitRight,
        StartComposition,
        UpdateCompositionRaw,
        UpdateCompositionChineseOne,
        UpdateCompositionChineseTwo,
        UpdateCompositionJapanese,
        UpdateCompositionKorean,
        UpdateCompositionWithRelativeSelection,
        CommitComposition,
        CommitChineseSample,
        DirectCommitChineseSample,
        CancelComposition,
        ToggleReadOnly,
        ReloadExternalSample,
        PreviewSaveText,
        ClearDeltaEvents,
        CreateTrackedRange,
        DemoTrackedRanges,
        ClearTrackedRanges,
        DemoMetadataLayers,
        QueryMetadataAtCursor,
        QueryMetadataLineWindow,
        ReplaceSearchMetadata,
        DiscardStaleMetadata,
        ClearMetadataLayers,
        ViewportFromCursor,
        ViewportUp,
        ViewportDown,
        ViewportGrow,
        ViewportShrink,
        ToggleViewportLineLimit,
        LoadLargeViewportSample,
        SnapshotViewportPreview,
        Quit,
    ]
);

const SAMPLE_TEXT: &str = "M11 Viewport Slicing / MetadataLayer / PositionMap / DeltaEvent 体验台\n\n\
英文区域：hello world\n\
中文输入区域：|\n\
日文输入区域：|\n\
韩文输入区域：|\n\
长行区域：0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ--这一行用于验证 viewport 的 max_line_chars 截断策略。\n\
\n\
继承 M6 Word movement：parseHTTPResponse user_id snake_case a+b == c && value != null\n\
\n\
M11 状态观察：BufferId / BufferState / DeltaEvent / PositionMap / TrackedRange / MetadataLayer / ViewportSlice。\n\
试试输入、删除、替换、Undo/Redo、composition、reload，并观察 tracked range、metadata range 与 viewport 可见行读取。";

const RELOAD_TEXT: &str = "M11 reload 后的新外部文本\n\n\
reload 会重建文本存储、清空 undo/redo history、selection 回到开头，并把当前文本设为 clean。\n\
你仍然可以继续输入、移动、多光标、composition、保存或再次 reload。\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastOperationKind {
    Info,
    Edit,
    Move,
    History,
    Composition,
    Lifecycle,
    Events,
    Tracked,
    Metadata,
    Viewport,
    Error,
}

impl LastOperationKind {
    fn label(self) -> &'static str {
        match self {
            Self::Info => "信息",
            Self::Edit => "编辑",
            Self::Move => "移动",
            Self::History => "历史",
            Self::Composition => "组合输入",
            Self::Lifecycle => "生命周期",
            Self::Events => "事件",
            Self::Tracked => "范围追踪",
            Self::Metadata => "元数据",
            Self::Viewport => "视口读取",
            Self::Error => "错误",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DemoMetadata {
    label: &'static str,
    detail: &'static str,
}

impl DemoMetadata {
    fn new(label: &'static str, detail: &'static str) -> Self {
        Self { label, detail }
    }
}

struct M11Testbed {
    buffer: Buffer,
    focus_handle: FocusHandle,
    active_unit: MovementUnit,
    last_operation: LastOperationKind,
    message: String,
    saved_label: String,
    last_preedit: String,
    tracked_ranges: Vec<TrackedRange>,
    last_tracked_update: String,
    metadata_layers: MetadataLayers<DemoMetadata>,
    last_metadata_update: String,
    viewport_start_line: Line,
    viewport_line_count: usize,
    viewport_max_line_chars: Option<usize>,
    last_viewport_update: String,
}

impl M11Testbed {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut buffer = initial_buffer().expect("M11 testbed sample text should be valid");
        buffer.mark_saved();

        let mut this = Self {
            buffer,
            focus_handle: cx.focus_handle(),
            active_unit: MovementUnit::Word,
            last_operation: LastOperationKind::Info,
            message: "M11 已就绪：PositionMap / DeltaEvent / pending events".to_string(),
            saved_label: "初始版本已保存".to_string(),
            last_preedit: String::new(),
            tracked_ranges: Vec::new(),
            last_tracked_update: "尚未创建 tracked range".to_string(),
            metadata_layers: MetadataLayers::new(),
            last_metadata_update: "尚未创建 metadata layer".to_string(),
            viewport_start_line: Line::ZERO,
            viewport_line_count: 6,
            viewport_max_line_chars: Some(48),
            last_viewport_update: "Viewport 从第 0 行开始，窗口 6 行，长行限制 48 字符".to_string(),
        };
        this.place_initial_cursor();
        this
    }

    fn place_initial_cursor(&mut self) {
        let text = self.buffer.text();
        let offset = find_char_offset(text.as_ref(), "中文输入区域：|")
            .map(|offset| CharOffset::new(offset.get() + "中文输入区域：".chars().count()))
            .unwrap_or(CharOffset::ZERO);
        let _ = self.buffer.set_selection(SelectionSet::caret(offset));
    }

    fn set_message(
        &mut self,
        kind: LastOperationKind,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.last_operation = kind;
        self.message = message.into();
        cx.notify();
    }

    fn handle_result<T>(
        &mut self,
        result: EngineResult<T>,
        ok_kind: LastOperationKind,
        ok_message: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Option<T> {
        match result {
            Ok(value) => {
                let message = self.append_range_sync_summary(ok_message.into());
                self.set_message(ok_kind, message, cx);
                Some(value)
            }
            Err(error) => {
                self.set_message(LastOperationKind::Error, format!("{error:?}"), cx);
                None
            }
        }
    }

    fn append_range_sync_summary(&mut self, mut message: String) -> String {
        if let Some(tracked_update) = self.sync_tracked_ranges_from_last_event() {
            message = format!("{message}；{tracked_update}");
        }
        if let Some(metadata_update) = self.sync_metadata_layers_from_last_event() {
            message = format!("{message}；{metadata_update}");
        }
        message
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let was_composing = self.buffer.is_composing();
        let selections = self.buffer.selection().clone();
        let count = selections.len();
        let result = self.buffer.insert_at_selections(selections, text);
        let suffix = if was_composing {
            "；已先取消 composition"
        } else {
            ""
        };
        self.handle_result(
            result,
            LastOperationKind::Edit,
            format!("插入 {:?} 到 {count} 个 selection{suffix}", text),
            cx,
        );
    }

    fn delete_backward(&mut self, cx: &mut Context<Self>) {
        let result = self
            .buffer
            .delete_backward_at_selections(self.buffer.selection().clone());
        self.handle_result(result, LastOperationKind::Edit, "Backspace 删除", cx);
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        let result = self
            .buffer
            .delete_forward_at_selections(self.buffer.selection().clone());
        self.handle_result(result, LastOperationKind::Edit, "Delete 删除", cx);
    }

    fn move_current(
        &mut self,
        direction: MovementDirection,
        unit: MovementUnit,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        if unit == MovementUnit::Grapheme && !extend {
            let current = self.buffer.selection().clone();
            if current
                .as_slice()
                .iter()
                .any(|selection| !selection.is_caret())
            {
                let primary_index = current.primary_index();
                let collapsed: Vec<Selection> = current
                    .as_slice()
                    .iter()
                    .copied()
                    .map(|selection| match direction {
                        MovementDirection::Previous => selection.collapse_to_start(),
                        MovementDirection::Next => selection.collapse_to_end(),
                    })
                    .collect();
                let result = self
                    .buffer
                    .set_selection(SelectionSet::new_with_primary(collapsed, primary_index));
                self.handle_result(result, LastOperationKind::Move, "已收起 selection", cx);
                return;
            }
        }

        let result = self.buffer.move_current_selection(direction, unit, extend);
        let msg = format!(
            "{} {} {}",
            if extend { "扩展选择" } else { "移动" },
            unit_label(unit),
            direction_label(direction),
        );
        self.handle_result(result, LastOperationKind::Move, msg, cx);
    }

    fn use_unit(&mut self, unit: MovementUnit, cx: &mut Context<Self>) {
        self.active_unit = unit;
        self.set_message(
            LastOperationKind::Info,
            format!("当前移动单位 = {}", unit_label(unit)),
            cx,
        );
    }

    fn reset_buffer(&mut self, cx: &mut Context<Self>) {
        match initial_buffer() {
            Ok(mut buffer) => {
                buffer.mark_saved();
                self.buffer = buffer;
                self.saved_label = "重置后的版本已保存".to_string();
                self.last_preedit.clear();
                self.tracked_ranges.clear();
                self.last_tracked_update = "重置后已清空 tracked ranges".to_string();
                self.metadata_layers = MetadataLayers::new();
                self.last_metadata_update = "重置后已清空 metadata layers".to_string();
                self.viewport_start_line = Line::ZERO;
                self.viewport_line_count = 6;
                self.viewport_max_line_chars = Some(48);
                self.last_viewport_update =
                    "重置后 viewport 回到第 0 行，窗口 6 行，长行限制 48 字符".to_string();
                self.place_initial_cursor();
                self.set_message(LastOperationKind::Info, "已重置示例文本", cx);
            }
            Err(error) => self.set_message(LastOperationKind::Error, format!("{error:?}"), cx),
        }
    }

    fn mark_saved(&mut self, cx: &mut Context<Self>) {
        self.buffer.mark_saved();
        self.saved_label = format!("已保存 v{}", self.buffer.version().get());
        self.set_message(LastOperationKind::Info, "已标记保存点", cx);
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        match self.buffer.undo() {
            Ok(Some(_)) => {
                let message = self.append_range_sync_summary("已 undo".to_string());
                self.set_message(LastOperationKind::History, message, cx);
            }
            Ok(None) => self.set_message(LastOperationKind::History, "没有可 undo 的历史", cx),
            Err(error) => self.set_message(LastOperationKind::Error, format!("{error:?}"), cx),
        }
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        match self.buffer.redo() {
            Ok(Some(_)) => {
                let message = self.append_range_sync_summary("已 redo".to_string());
                self.set_message(LastOperationKind::History, message, cx);
            }
            Ok(None) => self.set_message(LastOperationKind::History, "没有可 redo 的历史", cx),
            Err(error) => self.set_message(LastOperationKind::Error, format!("{error:?}"), cx),
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let end = self.buffer.len_chars();
        let selection = SelectionSet::new(vec![Selection::new(CharOffset::ZERO, end)]);
        let result = self.buffer.set_selection(selection);
        self.handle_result(result, LastOperationKind::Move, "已全选", cx);
    }

    fn collapse_to_primary(&mut self, cx: &mut Context<Self>) {
        let head = self.buffer.selection().primary().head();
        let result = self.buffer.set_selection(SelectionSet::caret(head));
        self.handle_result(
            result,
            LastOperationKind::Move,
            "已收起到 primary cursor",
            cx,
        );
    }

    fn demo_multi_cursor(&mut self, cx: &mut Context<Self>) {
        let text = self.buffer.text();
        let text = text.as_ref();
        let mut selections = Vec::new();

        for needle in [
            "hello",
            "parseHTTPResponse",
            "user_id",
            "a+b",
            "中文输入区域",
        ] {
            if let Some(offset) = find_char_offset(text, needle) {
                selections.push(Selection::caret(offset));
            }
        }

        if selections.is_empty() {
            self.set_message(LastOperationKind::Error, "没有找到 demo cursor anchor", cx);
            return;
        }

        let count = selections.len();
        let result = self.buffer.set_selection(SelectionSet::new(selections));
        self.handle_result(
            result,
            LastOperationKind::Move,
            format!("已创建 {count} 个 demo cursor"),
            cx,
        );
    }

    fn start_composition(&mut self, cx: &mut Context<Self>) {
        let original_count = self.buffer.selection().len();
        let result = self.buffer.start_composition();
        self.handle_result(
            result,
            LastOperationKind::Composition,
            format!("composition start；目标从 {original_count} 个 selection 降级到 primary"),
            cx,
        );
    }

    fn update_composition(
        &mut self,
        preedit: &str,
        selection: Option<CompositionSelection>,
        cx: &mut Context<Self>,
    ) {
        let result = self.buffer.update_composition(preedit, selection);
        self.last_preedit = preedit.to_string();
        self.handle_result(
            result,
            LastOperationKind::Composition,
            format!("composition update：preedit={preedit:?}"),
            cx,
        );
    }

    fn update_preedit_with_inner_selection(&mut self, cx: &mut Context<Self>) {
        let preedit = "输入法";
        let selection = CompositionSelection::new(CharOffset::new(1), CharOffset::new(2));
        self.update_composition(preedit, Some(selection), cx);
    }

    fn commit_composition(&mut self, cx: &mut Context<Self>) {
        let commit_text = self
            .buffer
            .composition()
            .map(|state| state.preedit_text().to_string())
            .filter(|text| !text.is_empty())
            .or_else(|| (!self.last_preedit.is_empty()).then(|| self.last_preedit.clone()))
            .unwrap_or_else(|| "你好".to_string());

        let result = self.buffer.commit_composition(&commit_text);
        self.handle_result(
            result,
            LastOperationKind::Composition,
            format!("composition commit：{commit_text:?}"),
            cx,
        );
    }

    fn commit_sample(&mut self, text: &str, cx: &mut Context<Self>) {
        let result = self.buffer.commit_composition(text);
        self.handle_result(
            result,
            LastOperationKind::Composition,
            format!("composition commit 示例：{text:?}"),
            cx,
        );
    }

    fn cancel_composition(&mut self, cx: &mut Context<Self>) {
        let result = self.buffer.cancel_composition();
        self.handle_result(
            result,
            LastOperationKind::Composition,
            "composition cancel",
            cx,
        );
    }

    fn toggle_read_only(&mut self, cx: &mut Context<Self>) {
        let next = !self.buffer.is_read_only();
        self.buffer.set_read_only(next);
        self.set_message(
            LastOperationKind::Lifecycle,
            format!("read_only={next}；只读时所有文本修改入口会返回 ReadOnly"),
            cx,
        );
    }

    fn reload_external_sample(&mut self, cx: &mut Context<Self>) {
        let result = self.buffer.reload_from_text(RELOAD_TEXT.to_string());
        if self
            .handle_result(
                result,
                LastOperationKind::Lifecycle,
                "已从外部文本 reload",
                cx,
            )
            .is_some()
        {
            self.saved_label = format!("reload clean v{}", self.buffer.version().get());
            self.last_preedit.clear();
            self.tracked_ranges.clear();
            self.last_tracked_update = "reload 后已清空 tracked ranges".to_string();
            self.metadata_layers = MetadataLayers::new();
            self.last_metadata_update = "reload 后已清空 metadata layers".to_string();
            self.viewport_start_line = Line::ZERO;
            self.last_viewport_update = "reload 后 viewport 回到第 0 行".to_string();
        }
    }

    fn preview_save_text(&mut self, cx: &mut Context<Self>) {
        match self.buffer.to_save_text(self.buffer.version()) {
            Ok(text) => {
                let preview = text.replace('\r', "\\r").replace('\n', "\\n");
                let preview: String = preview.chars().take(96).collect();
                self.set_message(
                    LastOperationKind::Lifecycle,
                    format!(
                        "save preview len={} chars={}：{}",
                        text.len(),
                        text.chars().count(),
                        preview
                    ),
                    cx,
                );
            }
            Err(error) => self.set_message(LastOperationKind::Error, format!("{error:?}"), cx),
        }
    }

    fn clear_delta_events(&mut self, cx: &mut Context<Self>) {
        let count = self.buffer.take_pending_events().len();
        self.set_message(
            LastOperationKind::Events,
            format!("已清空 {count} 个 pending DeltaEvent；last DeltaEvent 快照保留"),
            cx,
        );
    }

    fn create_tracked_range_from_primary(&mut self, cx: &mut Context<Self>) {
        let selection = *self.buffer.selection().primary();
        let range = selection.range();
        let stickiness = if selection.is_caret() {
            Stickiness::Expand
        } else {
            Stickiness::Never
        };
        let tracked_range = TrackedRange::from_range(self.buffer.version(), range, stickiness);
        self.tracked_ranges.push(tracked_range);
        self.last_tracked_update = format!(
            "新增 tracked range {}..{} stickiness={:?}",
            range.start().get(),
            range.end().get(),
            stickiness
        );
        self.set_message(
            LastOperationKind::Tracked,
            self.last_tracked_update.clone(),
            cx,
        );
    }

    fn create_demo_tracked_ranges(&mut self, cx: &mut Context<Self>) {
        let text = self.buffer.text();
        let text = text.as_ref();
        let version = self.buffer.version();
        let mut ranges = Vec::new();

        if let Some(range) = find_text_range(text, "hello world") {
            ranges.push(TrackedRange::from_range(version, range, Stickiness::Never));
        }
        if let Some(range) = find_text_range(text, "parseHTTPResponse") {
            ranges.push(TrackedRange::from_range(version, range, Stickiness::Never));
        }
        if let Some(offset) = find_char_offset(text, "user_id") {
            let range = TextRange::new(offset, offset)
                .expect("empty range from same offset should be valid");
            ranges.push(TrackedRange::from_range(version, range, Stickiness::Expand));
        }
        if let Some(offset) = find_char_offset(text, "韩文输入区域") {
            let range = TextRange::new(offset, offset)
                .expect("empty range from same offset should be valid");
            ranges.push(TrackedRange::from_range(version, range, Stickiness::Never));
        }

        if ranges.is_empty() {
            self.set_message(
                LastOperationKind::Error,
                "没有找到 demo tracked range anchor",
                cx,
            );
            return;
        }

        let count = ranges.len();
        self.tracked_ranges = ranges;
        self.last_tracked_update = format!("已创建 {count} 个 demo tracked range");
        self.set_message(
            LastOperationKind::Tracked,
            self.last_tracked_update.clone(),
            cx,
        );
    }

    fn clear_tracked_ranges(&mut self, cx: &mut Context<Self>) {
        let count = self.tracked_ranges.len();
        self.tracked_ranges.clear();
        self.last_tracked_update = format!("已清空 {count} 个 tracked range");
        self.set_message(
            LastOperationKind::Tracked,
            self.last_tracked_update.clone(),
            cx,
        );
    }

    fn create_demo_metadata_layers(&mut self, cx: &mut Context<Self>) {
        let text = self.buffer.text();
        let text = text.as_ref();
        let version = self.buffer.version();

        let mut search = MetadataLayer::with_kind(MetadataLayerKind::SearchMatch, version)
            .with_default_stickiness(Stickiness::Expand);
        for needle in ["hello", "输入区域", "MetadataLayer"] {
            if let Some(range) = find_text_range(text, needle) {
                let _ = search.insert(range, DemoMetadata::new("search", needle));
            }
        }

        let mut diagnostics = MetadataLayer::with_kind(MetadataLayerKind::Diagnostics, version)
            .with_default_stickiness(Stickiness::Never);
        if let Some(range) = find_text_range(text, "a+b == c") {
            let _ = diagnostics.insert_with_options(
                range,
                Stickiness::Never,
                TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion(),
                DemoMetadata::new("diagnostic", "模拟 warning：表达式需要检查"),
            );
        }
        if let Some(range) = find_text_range(text, "value != null") {
            let _ = diagnostics.insert_with_options(
                range,
                Stickiness::Never,
                TrackedRangeUpdatePolicy::invalidate_when_touched_by_deletion(),
                DemoMetadata::new("diagnostic", "模拟 info：空值判断"),
            );
        }

        let mut bookmarks = MetadataLayer::with_kind(MetadataLayerKind::Bookmark, version)
            .with_default_stickiness(Stickiness::Expand);
        for needle in ["中文输入区域", "韩文输入区域"] {
            if let Some(offset) = find_char_offset(text, needle) {
                let range = TextRange::new(offset, offset)
                    .expect("empty metadata range from same offset should be valid");
                let _ = bookmarks.insert(range, DemoMetadata::new("bookmark", needle));
            }
        }

        self.metadata_layers = MetadataLayers::from_layers([search, diagnostics, bookmarks]);
        self.last_metadata_update = format!(
            "已创建 demo metadata layers：{}",
            self.metadata_layer_counts()
        );
        self.set_message(
            LastOperationKind::Metadata,
            self.last_metadata_update.clone(),
            cx,
        );
    }

    fn query_metadata_at_cursor(&mut self, cx: &mut Context<Self>) {
        let head = self.buffer.selection().primary().head();
        let hits = self
            .metadata_layers
            .iter()
            .flat_map(|layer| {
                layer
                    .ranges_containing(head)
                    .map(move |range| format_metadata_hit(layer.kind(), range.metadata()))
            })
            .collect::<Vec<_>>();

        self.last_metadata_update = if hits.is_empty() {
            format!("cursor {} 没有命中 metadata", head.get())
        } else {
            format!("cursor {} 命中：{}", head.get(), hits.join(" | "))
        };
        self.set_message(
            LastOperationKind::Metadata,
            self.last_metadata_update.clone(),
            cx,
        );
    }

    fn query_metadata_line_window(&mut self, cx: &mut Context<Self>) {
        let head = self.buffer.selection().primary().head();
        let position = self.buffer.char_to_position(head).unwrap_or(Position::ZERO);
        let start = position.line();
        let end = Line::new((start.get() + 3).min(self.buffer.line_count()));
        let window = MetadataLineWindow::new(
            LineRange::new(start, end).expect("metadata line window must be ordered"),
        );

        let hits = self
            .metadata_layers
            .iter()
            .flat_map(|layer| {
                layer
                    .ranges_in_line_window(&self.buffer, window)
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |range| format_metadata_hit(layer.kind(), range.metadata()))
            })
            .collect::<Vec<_>>();

        self.last_metadata_update = format!(
            "line window {}..{} 命中 {} 个：{}",
            start.get(),
            end.get(),
            hits.len(),
            if hits.is_empty() {
                "none".to_string()
            } else {
                hits.join(" | ")
            }
        );
        self.set_message(
            LastOperationKind::Metadata,
            self.last_metadata_update.clone(),
            cx,
        );
    }

    fn replace_search_metadata_from_selection(&mut self, cx: &mut Context<Self>) {
        let selection = *self.buffer.selection().primary();
        let range = if selection.is_caret() {
            match line_text_range(&self.buffer, selection.head()) {
                Ok(range) => range,
                Err(error) => {
                    self.set_message(LastOperationKind::Error, format!("{error:?}"), cx);
                    return;
                }
            }
        } else {
            selection.range()
        };

        let ids = self.metadata_layers.replace_layer_ranges_with_options(
            MetadataLayerKind::SearchMatch,
            self.buffer.version(),
            [
                MetadataRangeSpec::new(range, DemoMetadata::new("search", "selection/line"))
                    .with_stickiness(Stickiness::Expand),
            ],
        );

        match ids {
            Ok(ids) => {
                self.last_metadata_update = format!(
                    "已批量替换 SearchMatch layer：{} 个 range，目标 {}..{}",
                    ids.len(),
                    range.start().get(),
                    range.end().get(),
                );
                self.set_message(
                    LastOperationKind::Metadata,
                    self.last_metadata_update.clone(),
                    cx,
                );
            }
            Err(error) => self.set_message(LastOperationKind::Error, format!("{error:?}"), cx),
        }
    }

    fn discard_stale_metadata_layers(&mut self, cx: &mut Context<Self>) {
        let removed = self.metadata_layers.discard_stale(self.buffer.version());
        self.last_metadata_update = format!(
            "丢弃 stale metadata layers：{} 个；剩余 {}",
            removed.len(),
            self.metadata_layers.len()
        );
        self.set_message(
            LastOperationKind::Metadata,
            self.last_metadata_update.clone(),
            cx,
        );
    }

    fn clear_metadata_layers(&mut self, cx: &mut Context<Self>) {
        let count = self.metadata_layers.len();
        self.metadata_layers = MetadataLayers::new();
        self.last_metadata_update = format!("已清空 {count} 个 metadata layer");
        self.set_message(
            LastOperationKind::Metadata,
            self.last_metadata_update.clone(),
            cx,
        );
    }

    fn current_viewport(&self) -> Viewport {
        let viewport = Viewport::new(self.viewport_start_line, self.viewport_line_count);
        match self.viewport_max_line_chars {
            Some(limit) => viewport.with_max_line_chars(limit),
            None => viewport,
        }
    }

    fn viewport_from_cursor(&mut self, cx: &mut Context<Self>) {
        match self
            .buffer
            .char_to_position(self.buffer.selection().primary().head())
        {
            Ok(position) => {
                self.viewport_start_line = position.line();
                self.update_viewport_message("已跳转 viewport 到主光标所在行", cx);
            }
            Err(error) => self.set_message(LastOperationKind::Error, format!("{error:?}"), cx),
        }
    }

    fn scroll_viewport(&mut self, delta: isize, cx: &mut Context<Self>) {
        let max_start = self.buffer.line_count();
        let next = if delta.is_negative() {
            self.viewport_start_line
                .get()
                .saturating_sub(delta.unsigned_abs())
        } else {
            self.viewport_start_line
                .get()
                .saturating_add(delta as usize)
                .min(max_start)
        };

        self.viewport_start_line = Line::new(next);
        self.update_viewport_message("已滚动 viewport", cx);
    }

    fn grow_viewport(&mut self, cx: &mut Context<Self>) {
        self.viewport_line_count = (self.viewport_line_count + 1).min(24);
        self.update_viewport_message("已增加 viewport 行数", cx);
    }

    fn shrink_viewport(&mut self, cx: &mut Context<Self>) {
        self.viewport_line_count = self.viewport_line_count.saturating_sub(1).max(1);
        self.update_viewport_message("已减少 viewport 行数", cx);
    }

    fn toggle_viewport_line_limit(&mut self, cx: &mut Context<Self>) {
        self.viewport_max_line_chars = match self.viewport_max_line_chars {
            Some(_) => None,
            None => Some(48),
        };
        self.update_viewport_message("已切换长行读取限制", cx);
    }

    fn load_large_viewport_sample(&mut self, cx: &mut Context<Self>) {
        let mut lines = Vec::with_capacity(1_204);
        lines.push(
            "M11 大文本 viewport 样本：用于滚动、跳转行、长行截断和只读 slice 体感验证".to_string(),
        );
        lines.push(
            "提示：Cmd-Alt-V 跳转到光标行，Cmd-Alt-↑/↓ 滚动，Cmd-Alt-L 切换长行限制".to_string(),
        );
        lines.push(format!("超长行：{}", "0123456789abcdef".repeat(40)));
        for line in 0..1_200 {
            lines.push(format!(
                "line-{line:04} | viewport slicing keeps reading logical visible lines"
            ));
        }

        let result = self.buffer.reload_from_text(lines.join("\n"));
        if self
            .handle_result(
                result,
                LastOperationKind::Viewport,
                "已加载 M11 大文本 viewport 样本",
                cx,
            )
            .is_some()
        {
            self.saved_label = format!(
                "large viewport sample clean v{}",
                self.buffer.version().get()
            );
            self.last_preedit.clear();
            self.tracked_ranges.clear();
            self.last_tracked_update = "加载大文本后已清空 tracked ranges".to_string();
            self.metadata_layers = MetadataLayers::new();
            self.last_metadata_update = "加载大文本后已清空 metadata layers".to_string();
            self.viewport_start_line = Line::ZERO;
            self.viewport_line_count = 10;
            self.viewport_max_line_chars = Some(64);
            self.last_viewport_update =
                "大文本样本已就绪：viewport 0..10，长行限制 64 字符".to_string();
        }
    }

    fn snapshot_viewport_preview(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.buffer.snapshot();
        match snapshot.slice_viewport(self.current_viewport()) {
            Ok(slice) => {
                let preview = slice
                    .lines()
                    .iter()
                    .map(|line| {
                        format!(
                            "{}:{}{}",
                            line.line().get(),
                            line.as_str().replace('\t', "⇥"),
                            if line.is_truncated() { "…" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                self.last_viewport_update = format!(
                    "Snapshot v{} / Buffer v{} / stale={} / viewport {}..{}：{}",
                    snapshot.version().get(),
                    self.buffer.version().get(),
                    self.buffer.is_snapshot_stale(&snapshot),
                    slice.line_range().start().get(),
                    slice.line_range().end().get(),
                    preview
                );
                self.set_message(
                    LastOperationKind::Viewport,
                    self.last_viewport_update.clone(),
                    cx,
                );
            }
            Err(error) => self.set_message(LastOperationKind::Error, format!("{error:?}"), cx),
        }
    }

    fn update_viewport_message(&mut self, prefix: &'static str, cx: &mut Context<Self>) {
        match self.buffer.slice_viewport(self.current_viewport()) {
            Ok(slice) => {
                let truncated = slice
                    .lines()
                    .iter()
                    .filter(|line| line.is_truncated())
                    .count();
                self.last_viewport_update = format!(
                    "{prefix}：Viewport {}..{} | visible lines={} | max_line_chars={} | truncated={truncated}",
                    slice.line_range().start().get(),
                    slice.line_range().end().get(),
                    slice.lines().len(),
                    self.viewport_limit_label(),
                );
                self.set_message(
                    LastOperationKind::Viewport,
                    self.last_viewport_update.clone(),
                    cx,
                );
            }
            Err(error) => self.set_message(LastOperationKind::Error, format!("{error:?}"), cx),
        }
    }

    fn sync_tracked_ranges_from_last_event(&mut self) -> Option<String> {
        if self.tracked_ranges.is_empty() {
            return None;
        }

        let event = self.buffer.last_delta_event()?.clone();
        if event.new_version != self.buffer.version()
            || self
                .tracked_ranges
                .iter()
                .any(|range| range.version() != event.old_version)
        {
            return None;
        }

        let updates = TrackedRange::map_all_through_delta_event_with_policy(
            self.tracked_ranges.clone(),
            &event,
            TrackedRangeUpdatePolicy::invalidate_when_fully_deleted(),
        )
        .ok()?;

        let before = self.tracked_ranges.len();
        self.tracked_ranges = updates
            .iter()
            .filter_map(|update| update.tracked_range())
            .collect();
        let invalidated = before - self.tracked_ranges.len();
        let summary = format_tracked_updates(&updates, invalidated);
        self.last_tracked_update = summary.clone();
        Some(summary)
    }

    fn sync_metadata_layers_from_last_event(&mut self) -> Option<String> {
        if self.metadata_layers.is_empty() {
            return None;
        }

        let event = self.buffer.last_delta_event()?.clone();
        if event.new_version != self.buffer.version() {
            return None;
        }

        let mut updated_layers = 0usize;
        let mut update_count = 0usize;
        let mut invalidated = 0usize;

        for layer in self.metadata_layers.iter_mut() {
            if layer.version() != event.old_version {
                continue;
            }

            let updates = layer.update_through_delta_event(&event).ok()?;
            updated_layers += 1;
            update_count += updates.len();
            invalidated += updates
                .iter()
                .filter(|update| matches!(update, MetadataRangeUpdate::Invalidated { .. }))
                .count();
        }

        if updated_layers == 0 {
            return None;
        }

        let summary = format!(
            "MetadataLayer 更新：layers={updated_layers} ranges={update_count} invalidated={invalidated}"
        );
        self.last_metadata_update = summary.clone();
        Some(summary)
    }

    fn primary_status(&self) -> String {
        let selection = self.buffer.selection().primary();
        let head = selection.head();
        let position = self.buffer.char_to_position(head).unwrap_or(Position::ZERO);
        let display_column = self
            .buffer
            .char_to_display_column(head)
            .unwrap_or(DisplayColumn::ZERO);
        let utf16 = self
            .buffer
            .char_to_utf16_position(head)
            .ok()
            .map(|position| {
                format!(
                    "UTF-16=({}, {})",
                    position.line().get(),
                    position.character().get()
                )
            })
            .unwrap_or_else(|| "UTF-16=<invalid>".to_string());

        format!(
            "主光标：head={} | line={} column={} display_column={} | {}",
            head.get(),
            position.line().get(),
            position.column().get(),
            display_column.get(),
            utf16
        )
    }

    fn boundary_preview(&self) -> String {
        let text = self.buffer.text();
        let text = text.as_ref();
        let head = self.buffer.selection().primary().head();
        let previous = self
            .buffer
            .previous_movement_boundary(head, self.active_unit)
            .unwrap_or(head);
        let next = self
            .buffer
            .next_movement_boundary(head, self.active_unit)
            .unwrap_or(head);
        let previous_text = slice_chars(text, previous, head).replace('\n', "⏎");
        let next_text = slice_chars(text, head, next).replace('\n', "⏎");

        format!(
            "移动预览：按「{}」移动｜左侧 {}..{} = {:?}｜右侧 {}..{} = {:?}",
            unit_label(self.active_unit),
            previous.get(),
            head.get(),
            previous_text,
            head.get(),
            next.get(),
            next_text
        )
    }

    fn composition_status(&self) -> String {
        match self.buffer.composition() {
            Some(state) => format!(
                "组合输入：进行中｜预编辑={:?}｜范围 {}..{}｜组合选区 {}..{}｜原始选区 {} 个",
                state.preedit_text(),
                state.range().start().get(),
                state.range().end().get(),
                state.selection().anchor().get(),
                state.selection().head().get(),
                state.original_selection().len(),
            ),
            None => "组合输入：未开始".to_string(),
        }
    }

    fn buffer_lifecycle_status(&self) -> String {
        let sync = self
            .buffer
            .last_synced_external_version()
            .map(|version| version.get().to_string())
            .unwrap_or_else(|| "未同步".to_string());
        format!(
            "缓冲区：id={}｜{}｜状态={:?}｜只读={}｜{}｜可直接关闭={}｜保存点 v{}｜最近保存 v{}｜外部同步={}",
            self.buffer.id().get(),
            buffer_kind_label(self.buffer.kind()),
            self.buffer.state(),
            bool_label(self.buffer.is_read_only()),
            dirty_label(self.buffer.is_dirty()),
            bool_label(self.buffer.can_close_without_prompt()),
            self.buffer.saved_version().get(),
            self.buffer.last_saved_version().get(),
            sync,
        )
    }

    fn loaded_text_status(&self) -> String {
        self.buffer
            .loaded_text_info()
            .map(|info| {
                format!(
                    "加载信息：编码={:?}｜BOM={:?}/{}｜非法 UTF-8={:?}/{}｜换行={:?}｜末尾换行={}",
                    info.encoding,
                    info.bom_policy,
                    info.had_bom,
                    info.invalid_utf8_policy,
                    info.had_invalid_utf8,
                    info.line_ending_style,
                    info.has_final_newline,
                )
            })
            .unwrap_or_else(|| {
                "加载信息：无（当前文本不是从外部字节加载，或已经重新加载）".to_string()
            })
    }

    fn delta_event_status(&self) -> String {
        let pending = self.buffer.pending_delta_event_count();
        let Some(event) = self.buffer.last_delta_event() else {
            return format!(
                "DeltaEvent：暂无；pending 队列={pending}；做一次输入 / Undo / Redo 后会生成事件"
            );
        };

        let changed_ranges = format_ranges(&event.changeset.changed_ranges());
        format!(
            "DeltaEvent：pending 队列={} | TransactionId={} | source={:?} | version {} -> {} | edits={} | changed ranges={}",
            pending,
            event.transaction_id.get(),
            event.source,
            event.old_version.get(),
            event.new_version.get(),
            event.delta.edits.len(),
            changed_ranges,
        )
    }

    fn position_map_status(&self) -> String {
        let Some(event) = self.buffer.last_delta_event() else {
            return "PositionMap：暂无最近事件可观察".to_string();
        };

        let first_old = event
            .delta
            .edits
            .as_slice()
            .first()
            .map(|edit| edit.range.start())
            .unwrap_or(CharOffset::ZERO);
        let primary_new = self.buffer.selection().primary().head();
        let before = event
            .position_map
            .map_old_position_with_affinity(first_old, Affinity::Before);
        let after = event
            .position_map
            .map_old_position_with_affinity(first_old, Affinity::After);
        let back = event.position_map.map_new_position(primary_new);

        format!(
            "PositionMap：segments={} | old {} Before -> {} | old {} After -> {} | 当前 head(new {}) -> old {}",
            event.position_map.len(),
            first_old.get(),
            format_offset_mapping(before),
            first_old.get(),
            format_offset_mapping(after),
            primary_new.get(),
            format_offset_mapping(back),
        )
    }

    fn tracked_range_status(&self) -> String {
        if self.tracked_ranges.is_empty() {
            return format!(
                "TrackedRange：0 个；{}；Cmd-T 从当前 selection 创建，Cmd-Shift-T 创建 demo ranges",
                self.last_tracked_update
            );
        }

        let ranges: Vec<String> = self
            .tracked_ranges
            .iter()
            .enumerate()
            .map(|(index, range)| {
                let text_preview = slice_chars(
                    self.buffer.text().as_ref(),
                    range.range().start(),
                    range.range().end(),
                )
                .replace('\n', "⏎");
                format!(
                    "#{} v{} {}..{} {:?} {:?}",
                    index + 1,
                    range.version().get(),
                    range.range().start().get(),
                    range.range().end().get(),
                    range.stickiness(),
                    text_preview,
                )
            })
            .collect();

        format!(
            "TrackedRange：{} 个 | {} | 最近：{}",
            self.tracked_ranges.len(),
            ranges.join(" | "),
            self.last_tracked_update,
        )
    }

    fn metadata_layer_counts(&self) -> String {
        if self.metadata_layers.is_empty() {
            return "none".to_string();
        }

        self.metadata_layers
            .iter()
            .map(|layer| format!("{}={}", metadata_kind_label(layer.kind()), layer.len()))
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn metadata_status(&self) -> String {
        let head = self.buffer.selection().primary().head();
        let cursor_hits = self
            .metadata_layers
            .iter()
            .flat_map(|layer| {
                layer
                    .ranges_containing(head)
                    .map(move |range| format_metadata_hit(layer.kind(), range.metadata()))
            })
            .collect::<Vec<_>>();

        format!(
            "MetadataLayer：layers={} | ranges={} | cursor hits={} | 最近：{}",
            self.metadata_layers.len(),
            self.metadata_layer_counts(),
            if cursor_hits.is_empty() {
                "none".to_string()
            } else {
                cursor_hits.join(" | ")
            },
            self.last_metadata_update,
        )
    }

    fn viewport_limit_label(&self) -> String {
        self.viewport_max_line_chars
            .map(|limit| format!("{limit} chars"))
            .unwrap_or_else(|| "off".to_string())
    }

    fn viewport_status(&self) -> String {
        match self.buffer.slice_viewport(self.current_viewport()) {
            Ok(slice) => {
                let truncated = slice
                    .lines()
                    .iter()
                    .filter(|line| line.is_truncated())
                    .count();
                format!(
                    "ViewportSlice：start={} | requested={} lines | actual={}..{} | max_line_chars={} | visible={} | truncated={} | last={}",
                    self.viewport_start_line.get(),
                    self.viewport_line_count,
                    slice.line_range().start().get(),
                    slice.line_range().end().get(),
                    self.viewport_limit_label(),
                    slice.lines().len(),
                    truncated,
                    self.last_viewport_update,
                )
            }
            Err(error) => format!("Viewport：读取失败 {error:?}"),
        }
    }

    fn viewport_preview_lines(&self) -> Vec<String> {
        match self.buffer.slice_viewport(self.current_viewport()) {
            Ok(slice) => {
                if slice.is_empty() {
                    return vec!["Viewport 当前为空窗口".to_string()];
                }

                let mut lines = vec![
                    "line  | full range | visible range | chars | bytes | cut | text".to_string(),
                    "------+------------+---------------+-------+-------+-----+----------------"
                        .to_string(),
                ];
                lines.extend(slice.lines().iter().map(|line| {
                    format!(
                        "{:>5} | {:>4}..{:<4} | {:>5}..{:<5} | {:>5} | {:>5} | {:>3} | {}",
                        line.line().get(),
                        line.full_range().start().get(),
                        line.full_range().end().get(),
                        line.visible_range().start().get(),
                        line.visible_range().end().get(),
                        line.visible_len_chars(),
                        line.visible_len_bytes(),
                        if line.is_truncated() { "yes" } else { "no" },
                        render_preview_text(line.as_str()),
                    )
                }));
                lines
            }
            Err(error) => vec![format!("Viewport 读取失败：{error:?}")],
        }
    }

    fn status_lines(&self) -> Vec<String> {
        let history = self.buffer.history_status();
        let snapshot = self.buffer.snapshot();
        let viewport = self
            .buffer
            .slice_viewport(self.current_viewport())
            .map(|slice| {
                format!(
                    "Viewport {}..{} / {} visible / {} truncated",
                    slice.line_range().start().get(),
                    slice.line_range().end().get(),
                    slice.lines().len(),
                    slice
                        .lines()
                        .iter()
                        .filter(|line| line.is_truncated())
                        .count(),
                )
            })
            .unwrap_or_else(|error| format!("Viewport error: {error:?}"));
        vec![
            format!(
                "M11 Viewport Testbed | {} | Buffer v{} | {} | {} lines | {} chars | {} bytes",
                self.last_operation.label(),
                self.buffer.version().get(),
                dirty_state_label(self.buffer.is_dirty()),
                self.buffer.line_count(),
                self.buffer.len_chars().get(),
                self.buffer.len_bytes(),
            ),
            format!(
                "Selection {} ranges / primary #{} | Undo {} / Redo {} | Snapshot v{} stale={} | {}",
                self.buffer.selection().len(),
                self.buffer.selection().primary_index(),
                history.undo_depth,
                history.redo_depth,
                snapshot.version().get(),
                self.buffer.is_snapshot_stale(&snapshot),
                self.saved_label,
            ),
            viewport,
            format!("最近操作：{}", self.message),
        ]
    }

    fn detail_lines(&self) -> Vec<String> {
        vec![
            self.buffer_lifecycle_status(),
            self.loaded_text_status(),
            self.primary_status(),
            self.boundary_preview(),
            self.composition_status(),
            self.delta_event_status(),
            self.position_map_status(),
            self.tracked_range_status(),
            self.metadata_status(),
            self.viewport_status(),
        ]
    }

    fn help_lines(&self) -> Vec<&'static str> {
        vec![
            "Edit：直接输入；Space / Tab / Enter；Backspace / Delete；Cmd-Z / Cmd-Shift-Z；Cmd-S 保存点；Cmd-R 重置",
            "Move：←/→ 字素；Alt 单词；Ctrl 标识符；Cmd 子词；Cmd-Alt 符号；Shift 扩展 selection",
            "Composition：Cmd-I start；Cmd-K/L/Y/J/O/U 更新 preedit；Cmd-Enter commit；Cmd-X cancel",
            "Ranges：Cmd-T tracked range；Cmd-Shift-T demo ranges；Cmd-G metadata layers；Cmd-Alt-G line window query",
            "Viewport：Cmd-Alt-V 跳到光标行；Cmd-Alt-↑/↓ 滚动；Cmd-Alt-= / Cmd-Alt-- 调整行数；Cmd-Alt-L max_line_chars；Cmd-Alt-B large sample；Cmd-Alt-P Snapshot",
        ]
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt {
            return;
        }

        let Some(key_char) = event.keystroke.key_char.as_ref() else {
            return;
        };

        if key_char.chars().any(|ch| ch.is_control()) {
            return;
        }

        if key_char != " " {
            self.insert_text(key_char, cx);
        }
    }

    fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(
            MovementDirection::Previous,
            MovementUnit::Grapheme,
            false,
            cx,
        );
    }

    fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Next, MovementUnit::Grapheme, false, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(
            MovementDirection::Previous,
            MovementUnit::Grapheme,
            true,
            cx,
        );
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Next, MovementUnit::Grapheme, true, cx);
    }

    fn move_word_left(&mut self, _: &MoveWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Previous, MovementUnit::Word, false, cx);
    }

    fn move_word_right(&mut self, _: &MoveWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Next, MovementUnit::Word, false, cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Previous, MovementUnit::Word, true, cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Next, MovementUnit::Word, true, cx);
    }

    fn move_identifier_left(
        &mut self,
        _: &MoveIdentifierLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(
            MovementDirection::Previous,
            MovementUnit::Identifier,
            false,
            cx,
        );
    }

    fn move_identifier_right(
        &mut self,
        _: &MoveIdentifierRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Next, MovementUnit::Identifier, false, cx);
    }

    fn select_identifier_left(
        &mut self,
        _: &SelectIdentifierLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(
            MovementDirection::Previous,
            MovementUnit::Identifier,
            true,
            cx,
        );
    }

    fn select_identifier_right(
        &mut self,
        _: &SelectIdentifierRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Next, MovementUnit::Identifier, true, cx);
    }

    fn move_subword_left(&mut self, _: &MoveSubwordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(
            MovementDirection::Previous,
            MovementUnit::Subword,
            false,
            cx,
        );
    }

    fn move_subword_right(&mut self, _: &MoveSubwordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Next, MovementUnit::Subword, false, cx);
    }

    fn select_subword_left(
        &mut self,
        _: &SelectSubwordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Previous, MovementUnit::Subword, true, cx);
    }

    fn select_subword_right(
        &mut self,
        _: &SelectSubwordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Next, MovementUnit::Subword, true, cx);
    }

    fn move_symbol_left(&mut self, _: &MoveSymbolLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Previous, MovementUnit::Symbol, false, cx);
    }

    fn move_symbol_right(&mut self, _: &MoveSymbolRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Next, MovementUnit::Symbol, false, cx);
    }

    fn select_symbol_left(&mut self, _: &SelectSymbolLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Previous, MovementUnit::Symbol, true, cx);
    }

    fn select_symbol_right(
        &mut self,
        _: &SelectSymbolRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Next, MovementUnit::Symbol, true, cx);
    }

    fn insert_space(&mut self, _: &InsertSpace, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_text(" ", cx);
    }

    fn insert_tab(&mut self, _: &InsertTab, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_text("\t", cx);
    }

    fn insert_newline(&mut self, _: &InsertNewline, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_text("\n", cx);
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_backward(cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_forward(cx);
    }

    fn undo_action(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo(cx);
    }

    fn redo_action(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.redo(cx);
    }

    fn save_action(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        self.mark_saved(cx);
    }

    fn reset_action(&mut self, _: &Reset, _: &mut Window, cx: &mut Context<Self>) {
        self.reset_buffer(cx);
    }

    fn demo_multi_cursor_action(
        &mut self,
        _: &DemoMultiCursor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.demo_multi_cursor(cx);
    }

    fn collapse_to_primary_action(
        &mut self,
        _: &CollapseToPrimary,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collapse_to_primary(cx);
    }

    fn select_all_action(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all(cx);
    }

    fn use_grapheme_mode(&mut self, _: &UseGraphemeMode, _: &mut Window, cx: &mut Context<Self>) {
        self.use_unit(MovementUnit::Grapheme, cx);
    }

    fn use_word_mode(&mut self, _: &UseWordMode, _: &mut Window, cx: &mut Context<Self>) {
        self.use_unit(MovementUnit::Word, cx);
    }

    fn use_identifier_mode(
        &mut self,
        _: &UseIdentifierMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.use_unit(MovementUnit::Identifier, cx);
    }

    fn use_subword_mode(&mut self, _: &UseSubwordMode, _: &mut Window, cx: &mut Context<Self>) {
        self.use_unit(MovementUnit::Subword, cx);
    }

    fn use_symbol_mode(&mut self, _: &UseSymbolMode, _: &mut Window, cx: &mut Context<Self>) {
        self.use_unit(MovementUnit::Symbol, cx);
    }

    fn move_active_unit_left(
        &mut self,
        _: &MoveActiveUnitLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Previous, self.active_unit, false, cx);
    }

    fn move_active_unit_right(
        &mut self,
        _: &MoveActiveUnitRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Next, self.active_unit, false, cx);
    }

    fn select_active_unit_left(
        &mut self,
        _: &SelectActiveUnitLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Previous, self.active_unit, true, cx);
    }

    fn select_active_unit_right(
        &mut self,
        _: &SelectActiveUnitRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Next, self.active_unit, true, cx);
    }

    fn start_composition_action(
        &mut self,
        _: &StartComposition,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_composition(cx);
    }

    fn update_composition_raw_action(
        &mut self,
        _: &UpdateCompositionRaw,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_composition("n", None, cx);
    }

    fn update_composition_chinese_one_action(
        &mut self,
        _: &UpdateCompositionChineseOne,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_composition("你", None, cx);
    }

    fn update_composition_chinese_two_action(
        &mut self,
        _: &UpdateCompositionChineseTwo,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_composition("你好", None, cx);
    }

    fn update_composition_japanese_action(
        &mut self,
        _: &UpdateCompositionJapanese,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_composition("にほん", None, cx);
    }

    fn update_composition_korean_action(
        &mut self,
        _: &UpdateCompositionKorean,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_composition("한글", None, cx);
    }

    fn update_composition_with_relative_selection_action(
        &mut self,
        _: &UpdateCompositionWithRelativeSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_preedit_with_inner_selection(cx);
    }

    fn commit_composition_action(
        &mut self,
        _: &CommitComposition,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_composition(cx);
    }

    fn commit_chinese_sample_action(
        &mut self,
        _: &CommitChineseSample,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_sample("你好", cx);
    }

    fn direct_commit_chinese_sample_action(
        &mut self,
        _: &DirectCommitChineseSample,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_sample("直接提交", cx);
    }

    fn cancel_composition_action(
        &mut self,
        _: &CancelComposition,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_composition(cx);
    }

    fn toggle_read_only_action(
        &mut self,
        _: &ToggleReadOnly,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_read_only(cx);
    }

    fn reload_external_sample_action(
        &mut self,
        _: &ReloadExternalSample,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reload_external_sample(cx);
    }

    fn preview_save_text_action(
        &mut self,
        _: &PreviewSaveText,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview_save_text(cx);
    }

    fn clear_delta_events_action(
        &mut self,
        _: &ClearDeltaEvents,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_delta_events(cx);
    }

    fn create_tracked_range_action(
        &mut self,
        _: &CreateTrackedRange,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_tracked_range_from_primary(cx);
    }

    fn demo_tracked_ranges_action(
        &mut self,
        _: &DemoTrackedRanges,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_demo_tracked_ranges(cx);
    }

    fn clear_tracked_ranges_action(
        &mut self,
        _: &ClearTrackedRanges,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_tracked_ranges(cx);
    }

    fn demo_metadata_layers_action(
        &mut self,
        _: &DemoMetadataLayers,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_demo_metadata_layers(cx);
    }

    fn query_metadata_at_cursor_action(
        &mut self,
        _: &QueryMetadataAtCursor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.query_metadata_at_cursor(cx);
    }

    fn query_metadata_line_window_action(
        &mut self,
        _: &QueryMetadataLineWindow,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.query_metadata_line_window(cx);
    }

    fn replace_search_metadata_action(
        &mut self,
        _: &ReplaceSearchMetadata,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_search_metadata_from_selection(cx);
    }

    fn discard_stale_metadata_action(
        &mut self,
        _: &DiscardStaleMetadata,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.discard_stale_metadata_layers(cx);
    }

    fn clear_metadata_layers_action(
        &mut self,
        _: &ClearMetadataLayers,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_metadata_layers(cx);
    }

    fn viewport_from_cursor_action(
        &mut self,
        _: &ViewportFromCursor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.viewport_from_cursor(cx);
    }

    fn viewport_up_action(&mut self, _: &ViewportUp, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_viewport(-(self.viewport_line_count as isize), cx);
    }

    fn viewport_down_action(&mut self, _: &ViewportDown, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_viewport(self.viewport_line_count as isize, cx);
    }

    fn viewport_grow_action(&mut self, _: &ViewportGrow, _: &mut Window, cx: &mut Context<Self>) {
        self.grow_viewport(cx);
    }

    fn viewport_shrink_action(
        &mut self,
        _: &ViewportShrink,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shrink_viewport(cx);
    }

    fn toggle_viewport_line_limit_action(
        &mut self,
        _: &ToggleViewportLineLimit,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_viewport_line_limit(cx);
    }

    fn load_large_viewport_sample_action(
        &mut self,
        _: &LoadLargeViewportSample,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_large_viewport_sample(cx);
    }

    fn snapshot_viewport_preview_action(
        &mut self,
        _: &SnapshotViewportPreview,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.snapshot_viewport_preview(cx);
    }
}

impl Focusable for M11Testbed {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for M11Testbed {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let decorated_text = decorate_text(
            self.buffer.text().as_ref(),
            self.buffer.selection(),
            self.buffer.composition(),
            &self.tracked_ranges,
            &self.metadata_layers,
        );
        let status_lines = self.status_lines();
        let detail_lines = self.detail_lines();
        let viewport_lines = self.viewport_preview_lines();
        let help_lines = self.help_lines();

        div()
            .id("m9-scroll-root")
            .key_context("M11Testbed")
            .track_focus(&self.focus_handle(cx))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::move_word_left))
            .on_action(cx.listener(Self::move_word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::move_identifier_left))
            .on_action(cx.listener(Self::move_identifier_right))
            .on_action(cx.listener(Self::select_identifier_left))
            .on_action(cx.listener(Self::select_identifier_right))
            .on_action(cx.listener(Self::move_subword_left))
            .on_action(cx.listener(Self::move_subword_right))
            .on_action(cx.listener(Self::select_subword_left))
            .on_action(cx.listener(Self::select_subword_right))
            .on_action(cx.listener(Self::move_symbol_left))
            .on_action(cx.listener(Self::move_symbol_right))
            .on_action(cx.listener(Self::select_symbol_left))
            .on_action(cx.listener(Self::select_symbol_right))
            .on_action(cx.listener(Self::insert_space))
            .on_action(cx.listener(Self::insert_tab))
            .on_action(cx.listener(Self::insert_newline))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::undo_action))
            .on_action(cx.listener(Self::redo_action))
            .on_action(cx.listener(Self::save_action))
            .on_action(cx.listener(Self::reset_action))
            .on_action(cx.listener(Self::demo_multi_cursor_action))
            .on_action(cx.listener(Self::collapse_to_primary_action))
            .on_action(cx.listener(Self::select_all_action))
            .on_action(cx.listener(Self::use_grapheme_mode))
            .on_action(cx.listener(Self::use_word_mode))
            .on_action(cx.listener(Self::use_identifier_mode))
            .on_action(cx.listener(Self::use_subword_mode))
            .on_action(cx.listener(Self::use_symbol_mode))
            .on_action(cx.listener(Self::move_active_unit_left))
            .on_action(cx.listener(Self::move_active_unit_right))
            .on_action(cx.listener(Self::select_active_unit_left))
            .on_action(cx.listener(Self::select_active_unit_right))
            .on_action(cx.listener(Self::start_composition_action))
            .on_action(cx.listener(Self::update_composition_raw_action))
            .on_action(cx.listener(Self::update_composition_chinese_one_action))
            .on_action(cx.listener(Self::update_composition_chinese_two_action))
            .on_action(cx.listener(Self::update_composition_japanese_action))
            .on_action(cx.listener(Self::update_composition_korean_action))
            .on_action(cx.listener(Self::update_composition_with_relative_selection_action))
            .on_action(cx.listener(Self::commit_composition_action))
            .on_action(cx.listener(Self::commit_chinese_sample_action))
            .on_action(cx.listener(Self::direct_commit_chinese_sample_action))
            .on_action(cx.listener(Self::cancel_composition_action))
            .on_action(cx.listener(Self::toggle_read_only_action))
            .on_action(cx.listener(Self::reload_external_sample_action))
            .on_action(cx.listener(Self::preview_save_text_action))
            .on_action(cx.listener(Self::clear_delta_events_action))
            .on_action(cx.listener(Self::create_tracked_range_action))
            .on_action(cx.listener(Self::demo_tracked_ranges_action))
            .on_action(cx.listener(Self::clear_tracked_ranges_action))
            .on_action(cx.listener(Self::demo_metadata_layers_action))
            .on_action(cx.listener(Self::query_metadata_at_cursor_action))
            .on_action(cx.listener(Self::query_metadata_line_window_action))
            .on_action(cx.listener(Self::replace_search_metadata_action))
            .on_action(cx.listener(Self::discard_stale_metadata_action))
            .on_action(cx.listener(Self::clear_metadata_layers_action))
            .on_action(cx.listener(Self::viewport_from_cursor_action))
            .on_action(cx.listener(Self::viewport_up_action))
            .on_action(cx.listener(Self::viewport_down_action))
            .on_action(cx.listener(Self::viewport_grow_action))
            .on_action(cx.listener(Self::viewport_shrink_action))
            .on_action(cx.listener(Self::toggle_viewport_line_limit_action))
            .on_action(cx.listener(Self::load_large_viewport_sample_action))
            .on_action(cx.listener(Self::snapshot_viewport_preview_action))
            .size_full()
            .overflow_y_scroll()
            .scrollbar_width(px(10.0))
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x111827))
            .text_color(white())
            .p(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .border_1()
                    .border_color(rgb(0x374151))
                    .bg(rgb(0x0f172a))
                    .p(px(12.0))
                    .text_size(px(14.0))
                    .line_height(px(22.0))
                    .child("状态 / Status")
                    .children(status_lines.into_iter()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .border_1()
                    .border_color(rgb(0x2563eb))
                    .bg(rgb(0x172554))
                    .p(px(12.0))
                    .text_size(px(13.0))
                    .line_height(px(20.0))
                    .child("快捷键 / Command Cheatsheet")
                    .children(help_lines.into_iter()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child("Editable Buffer")
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .border_1()
                                    .border_color(rgb(0xd1d5db))
                                    .bg(white())
                                    .text_color(black())
                                    .p(px(12.0))
                                    .text_size(px(18.0))
                                    .line_height(px(28.0))
                                    .child(decorated_text),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child("ViewportSlice")
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .border_1()
                                    .border_color(rgb(0x2563eb))
                                    .bg(rgb(0x0b1220))
                                    .p(px(12.0))
                                    .text_size(px(13.0))
                                    .line_height(px(20.0))
                                    .font_family("Menlo")
                                    .children(viewport_lines.into_iter()),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .border_1()
                    .border_color(rgb(0x374151))
                    .bg(rgb(0x0f172a))
                    .p(px(12.0))
                    .text_size(px(14.0))
                    .line_height(px(22.0))
                    .child("Debug Signals")
                    .children(detail_lines.into_iter()),
            )
    }
}

fn bool_label(value: bool) -> &'static str {
    if value { "是" } else { "否" }
}

fn dirty_label(is_dirty: bool) -> &'static str {
    if is_dirty { "已修改" } else { "干净" }
}

fn dirty_state_label(is_dirty: bool) -> &'static str {
    if is_dirty { "Dirty" } else { "Clean" }
}

fn unit_label(unit: MovementUnit) -> &'static str {
    match unit {
        MovementUnit::Grapheme => "字素",
        MovementUnit::Word => "单词",
        MovementUnit::Identifier => "标识符",
        MovementUnit::Subword => "子词",
        MovementUnit::Symbol => "符号",
    }
}

fn direction_label(direction: MovementDirection) -> &'static str {
    match direction {
        MovementDirection::Previous => "向左",
        MovementDirection::Next => "向右",
    }
}

fn initial_buffer() -> EngineResult<Buffer> {
    Buffer::from_loaded_text(
        BufferKind::file("/tmp/zom-engine-m11-testbed.txt"),
        SAMPLE_TEXT.as_bytes(),
        BufferConfig::default(),
    )
}

fn buffer_kind_label(kind: &BufferKind) -> String {
    match kind {
        BufferKind::File { path } => format!("文件({})", path.display()),
        BufferKind::Uri { uri } => format!("URI({uri})"),
        BufferKind::Untitled => "未命名".to_string(),
        BufferKind::Scratch => "临时缓冲区".to_string(),
    }
}

fn metadata_kind_label(kind: &MetadataLayerKind) -> String {
    match kind {
        MetadataLayerKind::SearchMatch => "搜索匹配".to_string(),
        MetadataLayerKind::Diagnostics => "诊断".to_string(),
        MetadataLayerKind::SyntaxHighlight => "语法高亮".to_string(),
        MetadataLayerKind::SemanticToken => "语义令牌".to_string(),
        MetadataLayerKind::Breakpoint => "断点".to_string(),
        MetadataLayerKind::Bookmark => "书签".to_string(),
        MetadataLayerKind::InlayHint => "内联提示".to_string(),
        MetadataLayerKind::CodeLens => "CodeLens".to_string(),
        MetadataLayerKind::Custom(name) => format!("Custom({name})"),
    }
}

fn format_metadata_hit(kind: &MetadataLayerKind, metadata: &DemoMetadata) -> String {
    format!(
        "{}:{}:{}",
        metadata_kind_label(kind),
        metadata.label,
        metadata.detail
    )
}

fn render_preview_text(text: &str) -> String {
    let rendered = text
        .replace('\r', "␍")
        .replace('\n', "⏎")
        .replace('\t', "⇥");

    if rendered.is_empty() {
        "∅".to_string()
    } else {
        rendered
    }
}

fn line_text_range(buffer: &Buffer, offset: CharOffset) -> EngineResult<TextRange> {
    let position = buffer.char_to_position(offset)?;
    let start = buffer.line_start(position.line())?;
    let next_line = Line::new(position.line().get() + 1);
    let end = if next_line.get() >= buffer.line_count() {
        buffer.len_chars()
    } else {
        buffer.line_start(next_line)?
    };
    Ok(TextRange::new(start, end)?)
}

fn find_char_offset(text: &str, needle: &str) -> Option<CharOffset> {
    let byte = text.find(needle)?;
    Some(CharOffset::new(text[..byte].chars().count()))
}

fn find_text_range(text: &str, needle: &str) -> Option<TextRange> {
    let start = find_char_offset(text, needle)?;
    let end = CharOffset::new(start.get() + needle.chars().count());
    TextRange::new(start, end).ok()
}

fn slice_chars(text: &str, start: CharOffset, end: CharOffset) -> String {
    let start = start.get().min(text.chars().count());
    let end = end.get().min(text.chars().count()).max(start);
    text.chars().skip(start).take(end - start).collect()
}

fn format_ranges(ranges: &[TextRange]) -> String {
    if ranges.is_empty() {
        return "[]".to_string();
    }

    let ranges: Vec<String> = ranges
        .iter()
        .map(|range| format!("{}..{}", range.start().get(), range.end().get()))
        .collect();
    format!("[{}]", ranges.join(", "))
}

fn format_offset_mapping(result: MappingResult<CharOffset>) -> String {
    match result {
        MappingResult::Mapped(offset) => format!("Mapped({})", offset.get()),
        MappingResult::Deleted(offset) => format!("Deleted({})", offset.get()),
        MappingResult::Collapsed(offset) => format!("Collapsed({})", offset.get()),
        MappingResult::Ambiguous(offset) => format!("Ambiguous({})", offset.get()),
    }
}

fn format_tracked_updates(updates: &[TrackedRangeUpdate], invalidated: usize) -> String {
    let mapped = updates
        .iter()
        .filter(|update| matches!(update, TrackedRangeUpdate::Mapped(_)))
        .count();
    let deleted = updates
        .iter()
        .filter(|update| matches!(update, TrackedRangeUpdate::Deleted(_)))
        .count();
    let collapsed = updates
        .iter()
        .filter(|update| matches!(update, TrackedRangeUpdate::Collapsed(_)))
        .count();
    format!(
        "TrackedRange 更新：mapped={mapped} deleted/shrunk={deleted} collapsed={collapsed} invalidated={invalidated}"
    )
}

fn push_visible_char(out: &mut String, c: char) {
    match c {
        '\t' => out.push('⇥'),
        ' ' => out.push('·'),
        '\r' => out.push('␍'),
        _ => out.push(c),
    }
}

fn decorate_text(
    text: &str,
    selections: &SelectionSet,
    composition: Option<&zom_engine::CompositionState>,
    tracked_ranges: &[TrackedRange],
    metadata_layers: &MetadataLayers<DemoMetadata>,
) -> String {
    let len = text.chars().count();
    let mut carets = vec![0usize; len + 1];
    let mut opens = vec![0usize; len + 1];
    let mut closes = vec![0usize; len + 1];
    let mut comp_opens = vec![0usize; len + 1];
    let mut comp_closes = vec![0usize; len + 1];
    let mut tracked_marks = vec![0usize; len + 1];
    let mut tracked_opens = vec![0usize; len + 1];
    let mut tracked_closes = vec![0usize; len + 1];
    let mut metadata_marks = vec![0usize; len + 1];
    let mut metadata_opens = vec![0usize; len + 1];
    let mut metadata_closes = vec![0usize; len + 1];

    for selection in selections.as_slice() {
        if selection.is_caret() {
            carets[selection.head().get().min(len)] += 1;
        } else {
            opens[selection.start().get().min(len)] += 1;
            closes[selection.end().get().min(len)] += 1;
        }
    }

    if let Some(state) = composition {
        comp_opens[state.range().start().get().min(len)] += 1;
        comp_closes[state.range().end().get().min(len)] += 1;
    }

    for tracked_range in tracked_ranges {
        let range = tracked_range.range();
        if range.is_empty() {
            tracked_marks[range.start().get().min(len)] += 1;
        } else {
            tracked_opens[range.start().get().min(len)] += 1;
            tracked_closes[range.end().get().min(len)] += 1;
        }
    }

    for layer in metadata_layers.iter() {
        for metadata_range in layer.iter() {
            let range = metadata_range.range();
            if range.is_empty() {
                metadata_marks[range.start().get().min(len)] += 1;
            } else {
                metadata_opens[range.start().get().min(len)] += 1;
                metadata_closes[range.end().get().min(len)] += 1;
            }
        }
    }

    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    out.push_str("图例：┃ caret，⟦selection⟧ range，〖composition preedit〗 range，‹tracked› range，◆ tracked caret，〔metadata〕 range，◇ metadata caret。所有 offset 都是 CharOffset；空格=·，Tab=⇥，CR=␍。\n\n");

    for index in 0..=len {
        for _ in 0..metadata_closes[index] {
            out.push('〕');
        }
        for _ in 0..tracked_closes[index] {
            out.push('›');
        }
        for _ in 0..closes[index] {
            out.push('⟧');
        }
        for _ in 0..comp_closes[index] {
            out.push('〗');
        }
        for _ in 0..comp_opens[index] {
            out.push('〖');
        }
        for _ in 0..opens[index] {
            out.push('⟦');
        }
        for _ in 0..tracked_opens[index] {
            out.push('‹');
        }
        for _ in 0..metadata_opens[index] {
            out.push('〔');
        }
        if carets[index] == 1 {
            out.push('┃');
        } else if carets[index] > 1 {
            out.push_str(&format!("┃×{}", carets[index]));
        }
        if tracked_marks[index] == 1 {
            out.push('◆');
        } else if tracked_marks[index] > 1 {
            out.push_str(&format!("◆×{}", tracked_marks[index]));
        }
        if metadata_marks[index] == 1 {
            out.push('◇');
        } else if metadata_marks[index] > 1 {
            out.push_str(&format!("◇×{}", metadata_marks[index]));
        }

        if index < len {
            push_visible_char(&mut out, chars[index]);
        }
    }

    out
}

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("left", MoveLeft, None),
        KeyBinding::new("right", MoveRight, None),
        KeyBinding::new("shift-left", SelectLeft, None),
        KeyBinding::new("shift-right", SelectRight, None),
        KeyBinding::new("alt-left", MoveWordLeft, None),
        KeyBinding::new("alt-right", MoveWordRight, None),
        KeyBinding::new("alt-shift-left", SelectWordLeft, None),
        KeyBinding::new("alt-shift-right", SelectWordRight, None),
        KeyBinding::new("ctrl-left", MoveIdentifierLeft, None),
        KeyBinding::new("ctrl-right", MoveIdentifierRight, None),
        KeyBinding::new("ctrl-shift-left", SelectIdentifierLeft, None),
        KeyBinding::new("ctrl-shift-right", SelectIdentifierRight, None),
        KeyBinding::new("cmd-left", MoveSubwordLeft, None),
        KeyBinding::new("cmd-right", MoveSubwordRight, None),
        KeyBinding::new("cmd-shift-left", SelectSubwordLeft, None),
        KeyBinding::new("cmd-shift-right", SelectSubwordRight, None),
        KeyBinding::new("cmd-alt-left", MoveSymbolLeft, None),
        KeyBinding::new("cmd-alt-right", MoveSymbolRight, None),
        KeyBinding::new("cmd-alt-shift-left", SelectSymbolLeft, None),
        KeyBinding::new("cmd-alt-shift-right", SelectSymbolRight, None),
        KeyBinding::new("space", InsertSpace, None),
        KeyBinding::new("tab", InsertTab, None),
        KeyBinding::new("enter", InsertNewline, None),
        KeyBinding::new("backspace", Backspace, None),
        KeyBinding::new("delete", Delete, None),
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("cmd-shift-z", Redo, None),
        KeyBinding::new("ctrl-y", Redo, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("cmd-r", Reset, None),
        KeyBinding::new("cmd-m", DemoMultiCursor, None),
        KeyBinding::new("escape", CollapseToPrimary, None),
        KeyBinding::new("cmd-a", SelectAll, None),
        KeyBinding::new("cmd-1", UseGraphemeMode, None),
        KeyBinding::new("cmd-2", UseWordMode, None),
        KeyBinding::new("cmd-3", UseIdentifierMode, None),
        KeyBinding::new("cmd-4", UseSubwordMode, None),
        KeyBinding::new("cmd-5", UseSymbolMode, None),
        KeyBinding::new("ctrl-alt-left", MoveActiveUnitLeft, None),
        KeyBinding::new("ctrl-alt-right", MoveActiveUnitRight, None),
        KeyBinding::new("ctrl-alt-shift-left", SelectActiveUnitLeft, None),
        KeyBinding::new("ctrl-alt-shift-right", SelectActiveUnitRight, None),
        KeyBinding::new("cmd-i", StartComposition, None),
        KeyBinding::new("cmd-k", UpdateCompositionRaw, None),
        KeyBinding::new("cmd-l", UpdateCompositionChineseOne, None),
        KeyBinding::new("cmd-y", UpdateCompositionChineseTwo, None),
        KeyBinding::new("cmd-j", UpdateCompositionJapanese, None),
        KeyBinding::new("cmd-o", UpdateCompositionKorean, None),
        KeyBinding::new("cmd-u", UpdateCompositionWithRelativeSelection, None),
        KeyBinding::new("cmd-enter", CommitComposition, None),
        KeyBinding::new("cmd-shift-enter", CommitChineseSample, None),
        KeyBinding::new("cmd-p", DirectCommitChineseSample, None),
        KeyBinding::new("cmd-x", CancelComposition, None),
        KeyBinding::new("cmd-b", ToggleReadOnly, None),
        KeyBinding::new("cmd-e", ReloadExternalSample, None),
        KeyBinding::new("cmd-shift-s", PreviewSaveText, None),
        KeyBinding::new("cmd-shift-e", ClearDeltaEvents, None),
        KeyBinding::new("cmd-t", CreateTrackedRange, None),
        KeyBinding::new("cmd-shift-t", DemoTrackedRanges, None),
        KeyBinding::new("cmd-alt-t", ClearTrackedRanges, None),
        KeyBinding::new("cmd-g", DemoMetadataLayers, None),
        KeyBinding::new("cmd-shift-g", QueryMetadataAtCursor, None),
        KeyBinding::new("cmd-alt-g", QueryMetadataLineWindow, None),
        KeyBinding::new("cmd-shift-m", ReplaceSearchMetadata, None),
        KeyBinding::new("cmd-alt-m", DiscardStaleMetadata, None),
        KeyBinding::new("cmd-alt-shift-m", ClearMetadataLayers, None),
        KeyBinding::new("cmd-alt-v", ViewportFromCursor, None),
        KeyBinding::new("cmd-alt-up", ViewportUp, None),
        KeyBinding::new("cmd-alt-down", ViewportDown, None),
        KeyBinding::new("cmd-alt-=", ViewportGrow, None),
        KeyBinding::new("cmd-alt--", ViewportShrink, None),
        KeyBinding::new("cmd-alt-l", ToggleViewportLineLimit, None),
        KeyBinding::new("cmd-alt-b", LoadLargeViewportSample, None),
        KeyBinding::new("cmd-alt-p", SnapshotViewportPreview, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
}

fn main() {
    Application::new().run(|cx: &mut App| {
        bind_keys(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1180.0), px(860.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(M11Testbed::new),
            )
            .expect("open M11 testbed window");

        window
            .update(cx, |view, window, cx| {
                window.focus(&view.focus_handle(cx));
                cx.activate(true);
            })
            .expect("focus M11 testbed window");
    });
}
