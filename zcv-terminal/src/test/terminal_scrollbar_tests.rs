use super::*;

#[test]
fn dragging_the_thumb_targets_the_matching_history_offset() {
    let handle = TerminalScrollHandle {
        state: Rc::new(RefCell::new(ScrollHandleState {
            line_height: px(16.),
            total_lines: 30,
            viewport_lines: 10,
            display_offset: 4,
        })),
        requested_display_offset: Rc::new(Cell::new(None)),
    };

    assert_eq!(handle.max_offset().y, px(320.));
    assert_eq!(handle.offset().y, px(-256.));

    handle.set_offset(point(px(0.), px(-80.)));
    assert_eq!(handle.take_requested_display_offset(), Some(15));
}
