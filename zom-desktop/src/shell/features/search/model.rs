use zom_command::{EditTarget, KeyContext, SearchOption};
use zom_workspace::BufferSearchOptions;

use crate::editor::text::{
    EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, ImeTarget, OwnedEditorTarget,
};
use crate::focus::{AppFocus, SearchField};
use crate::text_target::{TextTargetOwner, TextTargetQuery};

/// 从 focus 里抠出当前 search 字段；非 search focus 返回 `None`。
/// 各 `TextTargetOwner` 方法入口都先走它，避免散写 match。
fn search_field(focus: AppFocus) -> Option<SearchField> {
    focus.as_search()
}

/// 提供给 panel UI 渲染的快照。
///
/// 当前只有「输入框 + 选项 + 命中计数」，没有结果列表
/// ——所有匹配项都通过 EditorView 阶段 2 直接在 buffer 内高亮显示，panel 只是输入控制条。
#[derive(Clone, Debug, Default)]
pub(crate) struct SearchState {
    pub(crate) query: EditorSnapshot,
    pub(crate) replacement: EditorSnapshot,
    pub(crate) options: BufferSearchOptions,
    /// 当前命中 / 总命中数；`None` 表示尚无命中（query 空或未搜出结果）。
    /// 由活动 buffer 的 `BufferSearch` 同步推进或重新计算。
    pub(crate) hit_count: Option<HitCount>,
}

/// "3 / 27" 标签的数据来源。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct HitCount {
    /// 当前命中在结果集中的序号（1-based）。
    pub(crate) current: usize,
    /// 总命中数。
    pub(crate) total: usize,
}

/// 搜索栏的核心状态。
///
/// 算法层是**空的**：搜索 / 替换 / 导航全部委托给底层
/// `WorkspaceBuffer::BufferSearch`。`SearchModel` 只持有
/// UI 局部状态：两个输入框 + 选项 toggles + bar 可见标记。
pub(crate) struct SearchModel {
    query: OwnedEditorTarget,
    replacement: OwnedEditorTarget,
    options: BufferSearchOptions,
    /// 是否可见（mod-f 显示 / 收起的逻辑栅栏）。关闭后用来扣掉 buffer 高亮、
    /// 阻止后续 dispatch tail 的 `sync_active_buffer_search` 把命中复活。
    open: bool,
}

impl SearchModel {
    pub(crate) fn new() -> Self {
        Self {
            query: OwnedEditorTarget::new(),
            replacement: OwnedEditorTarget::new(),
            options: BufferSearchOptions::default(),
            open: false,
        }
    }

    pub(crate) fn state(&self) -> SearchState {
        SearchState {
            query: self.query.snapshot(EditorSnapshotRequest::single_line()),
            replacement: self
                .replacement
                .snapshot(EditorSnapshotRequest::single_line()),
            options: self.options,
            // 渲染层稍后从 active buffer 读 (current_hit_ordinal, hits.len()) 填进来。
            hit_count: None,
        }
    }

    /// 当前 query 文本——`App` 在每次命令派发后调它，把文本推进活动 buffer 的
    /// `BufferSearch`。
    pub(crate) fn query_text(&self) -> String {
        self.query.text()
    }

    /// 当前 replacement 文本，给 replace handler 用。
    pub(crate) fn replacement_text(&self) -> String {
        self.replacement.text()
    }

    /// 当前面板选项——`App` 在 sync 路径上调它把面板状态推到 `BufferSearch`。
    pub(crate) fn buffer_search_options(&self) -> BufferSearchOptions {
        self.options
    }

    pub(crate) fn toggle_option(&mut self, option: SearchOption) {
        match option {
            SearchOption::CaseSensitive => {
                self.options.case_sensitive = !self.options.case_sensitive
            }
            SearchOption::WholeWord => self.options.whole_word = !self.options.whole_word,
            SearchOption::Regex => self.options.regex = !self.options.regex,
        }
    }

    /// 内联搜索栏是否在屏。effects.rs 在 mod-f / Esc 翻转。
    pub(crate) fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn edit_target_for_focus(&mut self, focus: AppFocus) -> Option<EditTarget<'_>> {
        match search_field(focus)? {
            SearchField::Query => Some(self.query.as_edit_target()),
            SearchField::Replacement => Some(self.replacement.as_edit_target()),
        }
    }
}

/// 搜索面板两个输入框共用的按键解析栈：面板命令优先，其次文本编辑，最后全局兜底。
fn search_field_key_contexts(accepts_newline: bool) -> Vec<KeyContext> {
    vec![
        KeyContext::search_bar(),
        KeyContext::text_edit(accepts_newline, false),
        KeyContext::global(),
    ]
}

impl TextTargetQuery for SearchModel {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        search_field(focus).is_some()
    }

    fn snapshot(&self, focus: AppFocus) -> EditorSnapshot {
        let Some(field) = search_field(focus) else {
            return EditorSnapshot::default();
        };
        self.snapshot_for_field(field)
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        search_field_key_contexts(self.accepts_newline())
    }

    fn ime_query_target(&self, focus: AppFocus) -> Option<ImeQueryTarget<'_>> {
        self.ime_query_target_for_field(search_field(focus)?)
    }
}

impl TextTargetOwner for SearchModel {
    fn ime_target(&mut self, focus: AppFocus) -> Option<ImeTarget<'_>> {
        match search_field(focus)? {
            SearchField::Query => Some(self.query.as_ime_target()),
            SearchField::Replacement => Some(self.replacement.as_ime_target()),
        }
    }

    fn edit_target(&mut self, focus: AppFocus) -> Option<EditTarget<'_>> {
        self.edit_target_for_focus(focus)
    }
}

impl SearchModel {
    fn snapshot_for_field(&self, field: SearchField) -> EditorSnapshot {
        let request = EditorSnapshotRequest::single_line();
        match field {
            SearchField::Query => self.query.snapshot(request),
            SearchField::Replacement => self.replacement.snapshot(request),
        }
    }

    fn ime_query_target_for_field(&self, field: SearchField) -> Option<ImeQueryTarget<'_>> {
        match field {
            SearchField::Query => Some(self.query.as_ime_query_target()),
            SearchField::Replacement => Some(self.replacement.as_ime_query_target()),
        }
    }
}

// 搜索 / 替换导航的协调实现不在 SearchModel 里——见 [`super::coordinator`]。
// 那一层同时操作 panel 输入（query / replacement / options）+ active buffer 的 BufferSearch + active view 的 selection。
// SearchModel 只负责输入框状态。
