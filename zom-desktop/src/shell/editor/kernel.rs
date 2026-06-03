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
/// `soft_wrap` 字段用 `Rc<Cell<bool>>` 共享一个可变位：kernel 多次 clone 后仍指向同一状态，让运行时切换命令（`editor.toggle_soft_wrap`）改一处即可让主编辑区与任何嵌入处同步生效。
/// 其它能力开关都是不可变 builder 选项，不需要这层 indirection。
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
    pub(crate) fn multi_line() -> Self {
        Self {
            line_mode: EditorLineMode::MultiLine,
            gutter: false,
            vertical_scroll: false,
            viewport_sync: None,
            soft_wrap: Rc::new(Cell::new(false)),
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

    /// 开启软换行——按视口宽度把超长逻辑行拆成多条视觉行。
    /// 单行内核不应调用（无意义，单行只渲染 1 行）。
    #[allow(dead_code)] // builder 入口；运行时翻转走 [`soft_wrap_handle`]。
    pub(crate) fn with_soft_wrap(self) -> Self {
        self.soft_wrap.set(true);
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

    /// 运行时翻转软换行；任何 clone 都能改、所有 clone 都可见。
    /// 测试 / 直接持 kernel 的 caller 用；HostEffect 路径走 [`soft_wrap_handle`]
    /// 配合 `App::bind_main_soft_wrap`。
    #[allow(dead_code)]
    pub(crate) fn set_soft_wrap(&self, on: bool) {
        self.soft_wrap.set(on);
    }

    /// 拿到内部 `Rc<Cell<bool>>` 句柄——给宿主侧（[`crate::app::App`]）
    /// 跨界翻转用。
    ///
    /// HostEffect 路径无法直接持有 kernel（kernel 在渲染端），让 App 持
    /// 一份共享句柄是最薄的桥：toggle 命令打 effect，effect handler 翻这
    /// 个 cell，下一帧 kernel.soft_wrap() 就读到新值。
    pub(crate) fn soft_wrap_handle(&self) -> Rc<Cell<bool>> {
        Rc::clone(&self.soft_wrap)
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
