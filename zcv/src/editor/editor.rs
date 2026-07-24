//! 统一 Editor 的跨帧状态骨架。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Context, FocusHandle};
use zcv_engine::{Buffer, BufferConfig, SelectionSet};

use super::display_map::DisplayMap;
use super::scroll::ScrollManager;
use super::selection::SelectionHistory;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditorMode {
    SingleLine,
    AutoHeight {
        min_lines: usize,
        max_lines: Option<usize>,
    },
    Full,
}

pub(crate) struct Editor {
    buffer: Rc<RefCell<Buffer>>,
    display_map: DisplayMap,
    mode: EditorMode,
    selections: SelectionSet,
    selection_history: SelectionHistory,
    scroll_manager: ScrollManager,
    focus: FocusHandle,
}

impl Editor {
    pub(crate) fn single_line(cx: &mut Context<Self>) -> Self {
        let buffer = Buffer::scratch(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");
        Self::new(Rc::new(RefCell::new(buffer)), EditorMode::SingleLine, cx)
    }

    pub(crate) fn auto_height(
        min_lines: usize,
        max_lines: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Self {
        let buffer = Buffer::scratch(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");
        Self::new(
            Rc::new(RefCell::new(buffer)),
            EditorMode::AutoHeight {
                min_lines,
                max_lines,
            },
            cx,
        )
    }

    pub(crate) fn for_buffer(buffer: Rc<RefCell<Buffer>>, cx: &mut Context<Self>) -> Self {
        Self::new(buffer, EditorMode::Full, cx)
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn new(buffer: Rc<RefCell<Buffer>>, mode: EditorMode, cx: &mut Context<Self>) -> Self {
        let display_map = DisplayMap::new(buffer.borrow().snapshot());
        Self {
            buffer,
            display_map,
            mode,
            selections: SelectionSet::default(),
            selection_history: SelectionHistory::default(),
            scroll_manager: ScrollManager::default(),
            focus: cx.focus_handle(),
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, TestAppContext, point, px};
    use zcv_engine::{BufferConfig, ByteOffset, DisplayColumn, SelectionSet, TransactionId};

    use super::*;
    use crate::editor::display_map::{DisplayPoint, DisplayRow};

    #[gpui::test]
    fn editors_share_buffer_but_keep_view_state_independent(cx: &mut TestAppContext) {
        let buffer = Rc::new(RefCell::new(
            Buffer::scratch("abc".to_string(), BufferConfig::default())
                .expect("测试 Buffer 应能创建"),
        ));
        let first = cx.new(|cx| Editor::for_buffer(Rc::clone(&buffer), cx));
        let second = cx.new(|cx| Editor::for_buffer(Rc::clone(&buffer), cx));

        cx.update_entity(&first, |editor, _| {
            editor.selections = SelectionSet::caret(ByteOffset::new(1));
            editor
                .scroll_manager
                .set_anchor(DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(2)));
            editor.scroll_manager.set_offset(point(px(4.0), px(12.0)));
            editor.selection_history.record_transaction(
                TransactionId::new(1),
                SelectionSet::caret(ByteOffset::ZERO),
                editor.selections.clone(),
            );
            editor
                .buffer
                .borrow_mut()
                .insert(ByteOffset::new(3), "d")
                .expect("共享 Buffer 编辑应成功");
        });

        cx.read_entity(&second, |editor, _| {
            assert_eq!(editor.mode, EditorMode::Full);
            assert!(Rc::ptr_eq(&editor.buffer, &buffer));
            assert_eq!(editor.buffer.borrow().len_bytes(), ByteOffset::new(4));
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
            assert_eq!(
                editor.scroll_manager.anchor(),
                DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(2))
            );
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

        let single_buffer = cx.read_entity(&single_line, |editor, _| {
            assert_eq!(editor.mode, EditorMode::SingleLine);
            assert_eq!(editor.selections, SelectionSet::default());
            assert_eq!(
                editor.display_map.version(),
                editor.buffer.borrow().version()
            );
            let _focus = editor.focus_handle();
            Rc::clone(&editor.buffer)
        });
        let auto_height_buffer = cx.read_entity(&auto_height, |editor, _| {
            assert_eq!(
                editor.mode,
                EditorMode::AutoHeight {
                    min_lines: 2,
                    max_lines: Some(6),
                }
            );
            Rc::clone(&editor.buffer)
        });

        assert!(!Rc::ptr_eq(&single_buffer, &auto_height_buffer));
    }
}
