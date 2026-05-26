use zom_command::{EditTarget, SearchOption};
use zom_view::ViewSet;
use zom_workspace::Workspace;

use crate::shell::editor::{
    Editor, EditorSnapshot, ImeQueryTarget, ImeTarget, TextInputProfile, TextTargetId,
    TextTargetOwner, TextTargetQuery,
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
#[derive(Default)]
pub(crate) struct SearchModel {
    query: Editor,
    replacement: Editor,
    options: SearchOptions,
    active: Option<TextTargetId>,
}

impl SearchModel {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn state(&self) -> SearchState {
        SearchState {
            query: self.query.snapshot(),
            replacement: self.replacement.snapshot(),
            options: self.options,
            // 第一版恒为 None；BufferSearch 接入后从 active buffer 读
            // (current_hit_ordinal, hits.len()) 填进来。
            hit_count: None,
        }
    }

    /// P3 BufferSearch 调它拿当前 query 文本。第一版 panel 不主动搜索，所以
    /// 暂时没有内部调用——保留方法面，避免 BufferSearch 接入时改 owner API。
    #[allow(dead_code)]
    pub(crate) fn query_text(&self) -> String {
        self.query.text()
    }

    /// 同 [`Self::query_text`]：P3 替换接入时使用。
    #[allow(dead_code)]
    pub(crate) fn replacement_text(&self) -> String {
        self.replacement.text()
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
        if is_search_target(target) {
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

    pub(crate) fn edit_target(&mut self) -> Option<EditTarget<'_>> {
        match self.active {
            Some(TextTargetId::SearchQuery) => Some(self.query.as_edit_target()),
            Some(TextTargetId::SearchReplacement) => Some(self.replacement.as_edit_target()),
            _ => None,
        }
    }

    pub(crate) fn query_owner(&self) -> SearchFieldQuery<'_> {
        SearchFieldQuery {
            model: self,
            target: TextTargetId::SearchQuery,
        }
    }

    pub(crate) fn replacement_owner(&self) -> SearchFieldQuery<'_> {
        SearchFieldQuery {
            model: self,
            target: TextTargetId::SearchReplacement,
        }
    }

    pub(crate) fn active_owner(&mut self) -> SearchActiveOwner<'_> {
        SearchActiveOwner { model: self }
    }
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

    fn profile(&self) -> TextInputProfile {
        TextInputProfile::SearchField
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
        self.model.active.unwrap_or(TextTargetId::SearchQuery)
    }

    fn is_active(&self) -> bool {
        self.model.active()
    }

    fn snapshot(&self) -> EditorSnapshot {
        self.model.snapshot_for(self.target_id())
    }

    fn profile(&self) -> TextInputProfile {
        TextInputProfile::SearchField
    }

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        self.model
            .active
            .and_then(|target| self.model.ime_query_target_for(target))
    }
}

impl TextTargetOwner for SearchActiveOwner<'_> {
    fn ime_target(&mut self) -> Option<ImeTarget<'_>> {
        match self.model.active {
            Some(TextTargetId::SearchQuery) => Some(self.model.query.as_ime_target()),
            Some(TextTargetId::SearchReplacement) => Some(self.model.replacement.as_ime_target()),
            _ => None,
        }
    }

    fn edit_target(&mut self) -> Option<EditTarget<'_>> {
        self.model.edit_target()
    }
}

impl SearchModel {
    fn snapshot_for(&self, target: TextTargetId) -> EditorSnapshot {
        match target {
            TextTargetId::SearchQuery => self.query.snapshot(),
            TextTargetId::SearchReplacement => self.replacement.snapshot(),
            _ => EditorSnapshot::default(),
        }
    }

    fn ime_query_target_for(&self, target: TextTargetId) -> Option<ImeQueryTarget<'_>> {
        match target {
            TextTargetId::SearchQuery => Some(self.query.as_ime_query_target()),
            TextTargetId::SearchReplacement => Some(self.replacement.as_ime_query_target()),
            _ => None,
        }
    }
}

fn is_search_target(target: TextTargetId) -> bool {
    matches!(
        target,
        TextTargetId::SearchQuery | TextTargetId::SearchReplacement
    )
}

// =============================================================================
// 搜索 / 替换导航命令——第一版全部为空实现，等 BufferSearch 落地后接入。
// =============================================================================
//
// 命令骨架（HostEffect + zom-command 注册）保留，因此快捷键与按钮已经能派发；
// 只是 handler 还没有真正改 buffer 或 selection。BufferSearch 落地时：
//   1. 这些方法读 active view 的 buffer
//   2. 调 `buffer.search_mut()` 拿到（或刷新）`BufferSearch`
//   3. 推进 current hit / 应用替换 / reveal
//   4. panel 的 `hit_count` 改成从那里读，去掉 `state()` 里的 `None` 占位

impl SearchModel {
    pub(crate) fn find_next(&mut self, _workspace: &mut Workspace, _views: &mut ViewSet) {
        // TODO(P3 BufferSearch): 调 BufferSearch::next + reveal + 更新 selection。
    }

    pub(crate) fn find_previous(&mut self, _workspace: &mut Workspace, _views: &mut ViewSet) {
        // TODO(P3 BufferSearch): 调 BufferSearch::prev + reveal + 更新 selection。
    }

    pub(crate) fn replace_next(&mut self, _workspace: &mut Workspace, _views: &mut ViewSet) {
        // TODO(P3 BufferSearch): 替换当前 hit 并推进到下一个。
    }

    pub(crate) fn replace_all(&mut self, _workspace: &mut Workspace, _views: &mut ViewSet) {
        // TODO(P3 BufferSearch): 全量替换 active buffer 内的所有 hit。
    }
}
