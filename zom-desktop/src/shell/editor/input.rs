//! 编辑器输入宿主：把 GPUI 的文本输入回调接到编辑器 IME 能力。

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window};

use crate::app::App;

pub(crate) struct EditorInput {
    app: Rc<RefCell<App>>,
}

impl EditorInput {
    pub(crate) fn new(app: Rc<RefCell<App>>) -> Self {
        Self { app }
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
        self.app.borrow().ime_text_for_range_utf16(range)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.app
            .borrow()
            .ime_selected_range_utf16()
            .map(|(range, reversed)| UTF16Selection { range, reversed })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.app.borrow().ime_marked_range_utf16()
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.app.borrow_mut().ime_unmark() {
            eprintln!("IME unmark 失败：{error}");
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
        if let Err(error) = self.app.borrow_mut().ime_replace_text(range, text) {
            eprintln!("IME replace_text 失败：{error}");
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
        if let Err(error) =
            self.app
                .borrow_mut()
                .ime_replace_and_mark_text(range, new_text, new_selected_range)
        {
            eprintln!("IME replace_and_mark_text 失败：{error}");
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
        // 第一版：候选窗就贴在编辑区左上角；后续接入光标坐标再精修。
        Some(element_bounds)
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
