//! 可嵌入编辑器内核。
//!
//! `EditorKernel` 是「一个编辑器内核 + 若干正交能力开关」的显式承载点：
//! 调用方在装配 slot 时通过 builder 自己拼出想要的能力，内核负责把这些配置
//! 透传给 [`EditorElement`]。编辑器子系统不预设"主编辑区长什么样、单行框
//! 长什么样" —— 是否带行号 / 是否允许滚动 / 是否回写视口完全由调用方决定。
//!
//! 覆盖层（search overlay / reveal）走"数据驱动"：是否生效取决于 snapshot
//! 里是否带数据，不在内核上单独开关。调用方填了就画，没填就不画。

use std::cell::Cell;
use std::rc::Rc;

use gpui::FocusHandle;

use super::snapshot::EditorSnapshot;
use super::view::{EditorElement, EditorInputHook, EditorViewportSyncHook};

/// 编辑器的纵向承载模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EditorLineMode {
    /// 单行输入框：高度恰为一行，视口固定读取 1 行。
    SingleLine,
    /// 多行编辑面：撑满父容器，视口由滚动位置决定。
    MultiLine,
}

/// 可嵌入编辑器内核。
///
/// `soft_wrap` 是多行可嵌入编辑器的**固有能力**
/// ——`multi_line` 构造时必须传入由 [`crate::app::App`] 持有的共享 `Rc<Cell<bool>>`，
/// 所有多行嵌入点因此自动跟随同一份全局状态（设置面板 / 命令 / TOML save 翻一次，主编辑区与所有嵌入式编辑器同帧生效）。
/// 单行内核没有软换行语义，自带一个永不翻转的私有 cell——`soft_wrap()` 永远返回 `false`。
#[derive(Clone)]
pub(crate) struct EditorKernel {
    line_mode: EditorLineMode,
    gutter: bool,
    vertical_scroll: bool,
    viewport_sync: Option<EditorViewportSyncHook>,
    soft_wrap: Rc<Cell<bool>>,
}

impl EditorKernel {
    /// 单行内核起点：行模式固定 SingleLine，其它能力全关。
    /// 软换行对单行无意义，内部用一个私有 cell 永远保持 `false`。
    pub(crate) fn single_line() -> Self {
        Self {
            line_mode: EditorLineMode::SingleLine,
            gutter: false,
            vertical_scroll: false,
            viewport_sync: None,
            soft_wrap: Rc::new(Cell::new(false)),
        }
    }

    /// 多行内核起点：行模式固定 MultiLine，其它能力全关；通常再链
    /// `.with_gutter().with_vertical_scroll().with_viewport_sync(...)`。
    ///
    /// `soft_wrap` 是必填参数——多行嵌入式编辑器必须借用 App 的共享 cell，
    /// 不允许自家分配独立 cell，否则会跟全局软换行开关脱钩。
    pub(crate) fn multi_line(soft_wrap: Rc<Cell<bool>>) -> Self {
        Self {
            line_mode: EditorLineMode::MultiLine,
            gutter: false,
            vertical_scroll: false,
            viewport_sync: None,
            soft_wrap,
        }
    }

    pub(crate) fn with_gutter(mut self) -> Self {
        self.gutter = true;
        self
    }

    pub(crate) fn with_vertical_scroll(mut self) -> Self {
        self.vertical_scroll = true;
        self
    }

    /// 装一个视口写回钩子 —— 调用方拿到 element prepaint 测得的
    /// `(top_line, visible_line_count)`，自行决定怎么持久化（主编辑区把它写
    /// 进 `ViewportState`；单行框通常不装）。
    pub(crate) fn with_viewport_sync(mut self, hook: EditorViewportSyncHook) -> Self {
        self.viewport_sync = Some(hook);
        self
    }

    pub(crate) fn has_gutter(&self) -> bool {
        self.gutter
    }

    pub(crate) fn fills_viewport(&self) -> bool {
        matches!(self.line_mode, EditorLineMode::MultiLine)
    }

    pub(crate) fn allows_vertical_scroll(&self) -> bool {
        self.vertical_scroll
    }

    /// 当前是否启用软换行；clone 出去的 kernel 共享同一状态。
    pub(crate) fn soft_wrap(&self) -> bool {
        self.soft_wrap.get()
    }

    /// 从快照创建渲染元素。覆盖层（search / reveal）数据原样传给 element，
    /// element 看数据存在与否决定是否绘制。
    pub(crate) fn element(
        &self,
        snapshot: EditorSnapshot,
        focus: FocusHandle,
        input_handler_hook: EditorInputHook,
    ) -> EditorElement {
        let mut element = EditorElement::new(
            self.clone(),
            snapshot.lines,
            snapshot.total_lines,
            snapshot.viewport_start_line,
            snapshot.top_line,
            snapshot.selection,
            focus,
            input_handler_hook,
        )
        .reveal_if_some(snapshot.reveal)
        .decorations(snapshot.decorations);
        if let Some(hook) = self.viewport_sync.as_ref() {
            element = element.viewport_sync(Rc::clone(hook));
        }
        element
    }
}
