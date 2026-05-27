use zom_command::{EditTarget, KeyContext, SearchOption};
use zom_view::ViewSet;
use zom_workspace::{BufferSearchOptions, Workspace};

use crate::shell::editor::{
    EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, ImeTarget, OwnedEditorTarget,
    TextTargetId, TextTargetOwner, TextTargetQuery,
};

/// 提供给 panel UI 渲染的快照。
///
/// 第一版只有「输入框 + 选项 + 命中计数」，没有结果列表——所有匹配项都通过
/// EditorView 阶段 2 直接在 buffer 内高亮显示，panel 只是输入控制条。
#[derive(Clone, Debug, Default)]
pub(crate) struct SearchState {
    pub(crate) query: EditorSnapshot,
    pub(crate) replacement: EditorSnapshot,
    pub(crate) options: SearchOptions,
    /// 当前命中 / 总命中数；`None` 表示尚无命中（query 空或未搜出结果）。
    ///
    /// 第一版固定为 `None`——`BufferSearch` 落地前 panel 不主动跑搜索；
    /// 接入后由 `find_next/previous` 同步推进或重新计算。
    pub(crate) hit_count: Option<HitCount>,
}

/// "3 / 27" 标签的数据来源。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct HitCount {
    /// 当前 hit 在结果集中的序号（1-based）。
    pub(crate) current: usize,
    /// 总命中数。
    pub(crate) total: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SearchOptions {
    pub(crate) case_sensitive: bool,
    pub(crate) whole_word: bool,
    pub(crate) regex: bool,
}

/// 搜索面板的核心状态。
///
/// 第一版面板的算法层是**空的**：搜索 / 替换 / 导航全部委托给底层
/// `WorkspaceBuffer::BufferSearch`（P3 待落地）。`SearchModel` 只持有
/// UI 局部状态：两个输入框 + 选项 toggles + 当前活动输入框。
pub(crate) struct SearchModel {
    query_id: TextTargetId,
    replacement_id: TextTargetId,
    query: OwnedEditorTarget,
    replacement: OwnedEditorTarget,
    options: SearchOptions,
    active: Option<TextTargetId>,
    /// 面板是否可见（mod-f 显示 / 收起的逻辑栅栏，与 `active` 不同：active 表示
    /// 哪个输入框获得焦点，可能在 query / replacement 之间切换；panel_open 是
    /// "整个搜索面板还在不在屏上"——关闭后用来扣掉 buffer 高亮、阻止后续
    /// dispatch tail 的 `sync_active_buffer_search` 把命中复活）。
    panel_open: bool,
}

impl SearchModel {
    pub(crate) fn new(query_id: TextTargetId, replacement_id: TextTargetId) -> Self {
        Self {
            query_id,
            replacement_id,
            query: OwnedEditorTarget::new(),
            replacement: OwnedEditorTarget::new(),
            options: SearchOptions::default(),
            active: None,
            panel_open: false,
        }
    }

    fn is_search_target(&self, target: TextTargetId) -> bool {
        target == self.query_id || target == self.replacement_id
    }

    pub(crate) fn state(&self) -> SearchState {
        SearchState {
            query: self.query.snapshot(EditorSnapshotRequest::single_line()),
            replacement: self
                .replacement
                .snapshot(EditorSnapshotRequest::single_line()),
            options: self.options,
            // 第一版恒为 None；BufferSearch 接入后从 active buffer 读
            // (current_hit_ordinal, hits.len()) 填进来。
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

    /// 转成 zom-workspace 的 `BufferSearchOptions`——一字段映射，分离两边的命名
    /// 空间。`App` 在 sync 路径上调它把面板状态推到 `BufferSearch`。
    pub(crate) fn buffer_search_options(&self) -> BufferSearchOptions {
        BufferSearchOptions {
            case_sensitive: self.options.case_sensitive,
            whole_word: self.options.whole_word,
            regex: self.options.regex,
        }
    }

    pub(crate) fn toggle_option(
        &mut self,
        option: SearchOption,
        _workspace: &Workspace,
        _views: &ViewSet,
    ) {
        match option {
            SearchOption::CaseSensitive => {
                self.options.case_sensitive = !self.options.case_sensitive
            }
            SearchOption::WholeWord => self.options.whole_word = !self.options.whole_word,
            SearchOption::Regex => self.options.regex = !self.options.regex,
        }
        // BufferSearch 接入后：选项变化要触发 active buffer 的 BufferSearch
        // 同步重跑（与 query 变化同处理）；当前空实现。
    }

    pub(crate) fn activate(&mut self, target: TextTargetId) {
        if self.is_search_target(target) {
            self.active = Some(target);
        }
    }

    pub(crate) fn deactivate(&mut self, target: TextTargetId) {
        if self.active == Some(target) {
            self.active = None;
        }
    }

    pub(crate) fn active(&self) -> bool {
        self.active.is_some()
    }

    /// 整个搜索面板是否在屏上。`activate` / `deactivate` 只标输入框焦点；这条
    /// 由 effects.rs 在 `show_panel` / `hide_panel` 调用同步。
    pub(crate) fn set_panel_open(&mut self, open: bool) {
        self.panel_open = open;
    }

    pub(crate) fn panel_open(&self) -> bool {
        self.panel_open
    }

    pub(crate) fn edit_target(&mut self) -> Option<EditTarget<'_>> {
        let active = self.active?;
        if active == self.query_id {
            Some(self.query.as_edit_target())
        } else if active == self.replacement_id {
            Some(self.replacement.as_edit_target())
        } else {
            None
        }
    }

    pub(crate) fn query_owner(&self) -> SearchFieldQuery<'_> {
        SearchFieldQuery {
            model: self,
            target: self.query_id,
        }
    }

    pub(crate) fn replacement_owner(&self) -> SearchFieldQuery<'_> {
        SearchFieldQuery {
            model: self,
            target: self.replacement_id,
        }
    }

    pub(crate) fn active_owner(&mut self) -> SearchActiveOwner<'_> {
        SearchActiveOwner { model: self }
    }
}

/// 搜索面板输入框（query / replacement 通用）的按键解析栈：
/// 面板命令优先，其次文本编辑，最后全局兜底。
fn search_field_key_contexts(accepts_newline: bool) -> Vec<KeyContext> {
    vec![
        KeyContext::search_panel(),
        KeyContext::text_edit(accepts_newline, false),
        KeyContext::global(),
    ]
}

pub(crate) struct SearchFieldQuery<'a> {
    model: &'a SearchModel,
    target: TextTargetId,
}

pub(crate) struct SearchActiveOwner<'a> {
    model: &'a mut SearchModel,
}

impl TextTargetQuery for SearchFieldQuery<'_> {
    fn target_id(&self) -> TextTargetId {
        self.target
    }

    fn is_active(&self) -> bool {
        self.model.active == Some(self.target)
    }

    fn snapshot(&self) -> EditorSnapshot {
        self.model.snapshot_for(self.target)
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        search_field_key_contexts(self.accepts_newline())
    }

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        if self.is_active() {
            self.model.ime_query_target_for(self.target)
        } else {
            None
        }
    }
}

impl TextTargetQuery for SearchActiveOwner<'_> {
    fn target_id(&self) -> TextTargetId {
        // active 为空时退到 query 字段——只在无任何输入框聚焦的边界态被读到，
        // 此时整对 owner 都不算 active（is_active() 返回 false），target_id 只作
        // 调试 / 兜底用。
        self.model.active.unwrap_or(self.model.query_id)
    }

    fn is_active(&self) -> bool {
        self.model.active()
    }

    fn snapshot(&self) -> EditorSnapshot {
        self.model.snapshot_for(self.target_id())
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        search_field_key_contexts(self.accepts_newline())
    }

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        self.model
            .active
            .and_then(|target| self.model.ime_query_target_for(target))
    }
}

impl TextTargetOwner for SearchActiveOwner<'_> {
    fn ime_target(&mut self) -> Option<ImeTarget<'_>> {
        let active = self.model.active?;
        if active == self.model.query_id {
            Some(self.model.query.as_ime_target())
        } else if active == self.model.replacement_id {
            Some(self.model.replacement.as_ime_target())
        } else {
            None
        }
    }

    fn edit_target(&mut self) -> Option<EditTarget<'_>> {
        self.model.edit_target()
    }
}

impl SearchModel {
    fn snapshot_for(&self, target: TextTargetId) -> EditorSnapshot {
        let request = EditorSnapshotRequest::single_line();
        if target == self.query_id {
            self.query.snapshot(request)
        } else if target == self.replacement_id {
            self.replacement.snapshot(request)
        } else {
            EditorSnapshot::default()
        }
    }

    fn ime_query_target_for(&self, target: TextTargetId) -> Option<ImeQueryTarget<'_>> {
        if target == self.query_id {
            Some(self.query.as_ime_query_target())
        } else if target == self.replacement_id {
            Some(self.replacement.as_ime_query_target())
        } else {
            None
        }
    }
}

// 搜索 / 替换导航命令的算法实现不在 SearchModel 里——它们直接在 `App` 上，
// 因为需要同时操作 panel 输入（query / replacement / options）+ active buffer
// 的 BufferSearch + active view 的 selection。SearchModel 只负责输入框状态。
