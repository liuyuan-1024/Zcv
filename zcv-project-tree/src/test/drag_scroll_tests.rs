use gpui::px;

#[test]
fn project_tree_drag_scroll_offset_uses_negative_list_range() {
    let max_offset = px(100.);
    let current = px(-40.);
    let next = (current + px(12.)).min(px(0.)).max(-max_offset);
    assert_eq!(next, px(-28.));

    let at_top = (px(-4.) + px(12.)).min(px(0.)).max(-max_offset);
    assert_eq!(at_top, px(0.));

    let at_bottom = (px(-96.) - px(12.)).min(px(0.)).max(-max_offset);
    assert_eq!(at_bottom, px(-100.));
}
