use gpui::{AppContext, TestAppContext};

use super::*;

#[gpui::test]
fn toggle_flips_marker_state(cx: &mut TestAppContext) {
    let button = cx.new(|_| HarnessButton::new());
    cx.read_entity(&button, |button, _| assert!(!button.harness_on));
    cx.update_entity(&button, |button, cx| button.toggle(cx));
    cx.read_entity(&button, |button, _| assert!(button.harness_on));
    cx.update_entity(&button, |button, cx| button.toggle(cx));
    cx.read_entity(&button, |button, _| assert!(!button.harness_on));
}
