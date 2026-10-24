use std::{cell::Cell, rc::Rc};

use gpui::{Menu, MenuItem, TestAppContext};
use zcv_actions::{About, CheckForUpdates};

use super::*;

#[gpui::test]
fn application_menu_restores_focus_after_keyboard_close(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_menus([Menu::new("帮助").items([
            MenuItem::action("关于 Zcv", About),
            MenuItem::action("检查更新", CheckForUpdates),
        ])]);
    });
    let (menu, visual) =
        cx.add_window_view(|_, cx| ApplicationMenu::new(PopoverLayer::default(), cx));
    let previous = visual.update(|_, cx| cx.focus_handle());
    visual.update(|window, cx| window.focus(&previous, cx));
    menu.update_in(visual, |menu, window, cx| menu.toggle(window, cx));
    visual.run_until_parked();
    menu.update_in(visual, |menu, window, cx| {
        menu.close(&CloseMenu, window, cx)
    });
    visual.update(|window, _| assert!(previous.is_focused(window)));
}

#[gpui::test]
fn application_menu_confirms_the_selected_action(cx: &mut TestAppContext) {
    let invoked = Rc::new(Cell::new(false));
    let invoked_by_action = invoked.clone();
    cx.update(|cx| {
        cx.on_action(move |_: &About, _| invoked_by_action.set(true));
        cx.set_menus([Menu::new("帮助").items([
            MenuItem::action("关于 Zcv", About),
            MenuItem::action("检查更新", CheckForUpdates),
        ])]);
    });
    let (menu, visual) =
        cx.add_window_view(|_, cx| ApplicationMenu::new(PopoverLayer::default(), cx));
    menu.update_in(visual, |menu, window, cx| menu.toggle(window, cx));
    menu.update_in(visual, |menu, window, cx| {
        menu.confirm(&ConfirmItem, window, cx)
    });
    visual.run_until_parked();
    assert!(invoked.get());
}
