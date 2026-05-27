//! 编辑器输入宿主：把 GPUI 的文本输入回调接到编辑器 IME 能力。

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window, point, size,
};

use crate::app::App;
use crate::shell::platform::clipboard::GpuiClipboardScope;

/// primary caret 在 element 内的相对位置 + 行高 —— 系统 IME 候选窗定位用。
///
/// 每帧 paint 由 [`crate::shell::editor::view::EditorElement`] 通过 input hook 写入；GPUI
/// 调 [`EditorInput::bounds_for_range`] 查 caret 屏幕 rect 时读取，加上
/// `element_bounds.origin` 即转成绝对坐标。
///
/// 这是 GPUI 官方 input 示例的同款模式（`last_layout` + `last_bounds`）：渲染
/// 期算出的几何信息暂存到 input 实体，输入法回调时反查。
#[derive(Clone, Copy, Debug)]
pub(crate) struct CaretLayout {
    /// caret 左上角相对 element bounds.origin 的偏移（已吸收 scroll + gutter）。
    pub relative: Point<Pixels>,
    pub line_height: Pixels,
}

pub(crate) struct EditorInput {
    app: Rc<RefCell<App>>,
    caret_layout: Option<CaretLayout>,
}

impl EditorInput {
    pub(crate) fn new(app: Rc<RefCell<App>>) -> Self {
        Self {
            app,
            caret_layout: None,
        }
    }

    /// element paint 阶段调用：把 primary caret 几何信息存进来，供 IME 反查。
    pub(crate) fn set_caret_layout(&mut self, layout: Option<CaretLayout>) {
        self.caret_layout = layout;
    }
}

impl EntityInputHandler for EditorInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let focus = self.app.borrow().focus().current();
        self.app
            .borrow()
            .with_router(|router| router.text_for_range_utf16(focus, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let focus = self.app.borrow().focus().current();
        self.app
            .borrow()
            .with_router(|router| router.selected_range_utf16(focus))
            .map(|(range, reversed)| UTF16Selection { range, reversed })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let focus = self.app.borrow().focus().current();
        self.app
            .borrow()
            .with_router(|router| router.marked_range_utf16(focus))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.app.borrow().focus().current();
        {
            // IME 路径也可能触发命令派发（commit → editor.ime_commit），需为
            // 期间的剪贴板读写借出 cx。
            let _clip = GpuiClipboardScope::enter(&*cx);
            if let Err(error) = self.app.borrow_mut().ime_unmark_for(focus) {
                eprintln!("IME unmark 失败：{error}");
            }
        }
        window.refresh();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = self.app.borrow().focus().current();
        {
            let _clip = GpuiClipboardScope::enter(&*cx);
            if let Err(error) = self
                .app
                .borrow_mut()
                .ime_replace_text_for(focus, range, text)
            {
                eprintln!("IME replace_text 失败：{error}");
            }
        }
        window.refresh();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = self.app.borrow().focus().current();
        {
            let _clip = GpuiClipboardScope::enter(&*cx);
            if let Err(error) = self.app.borrow_mut().ime_replace_and_mark_text_for(
                focus,
                range,
                new_text,
                new_selected_range,
            ) {
                eprintln!("IME replace_and_mark_text 失败：{error}");
            }
        }
        window.refresh();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // 返回 primary caret 的绝对屏幕 rect —— 系统 IME 据此把候选窗放到 caret
        // 正下方。无 caret 时返回 None，让系统走默认（落在窗口左上角，明显
        // 比"贴在编辑区左上角"好辨认是哪里有问题）。
        let layout = self.caret_layout?;
        let origin = point(
            element_bounds.origin.x + layout.relative.x,
            element_bounds.origin.y + layout.relative.y,
        );
        Some(Bounds {
            origin,
            size: size(Pixels::from(2.0), layout.line_height),
        })
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
