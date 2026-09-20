use super::*;
use gpui::TestAppContext;

#[gpui::test]
fn status_items_start_without_projection(cx: &mut TestAppContext) {
    let cursor = cx.new(|_| CursorPosition::new());
    let language = cx.new(|_| ActiveBufferLanguage::new());
    cx.read_entity(&cursor, |item, _| assert!(item.cursor_text.is_empty()));
    cx.read_entity(&language, |item, _| assert!(item.language.is_empty()));
}
