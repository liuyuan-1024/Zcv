use std::{cell::Cell, rc::Rc};

use gpui::{
    Context, IntoElement, Menu, MenuItem, Render, TestAppContext, VisualTestContext, Window, px,
    size,
};
use zcv_actions::{About, CheckForUpdates, menu};

use super::*;

struct TestRoot;

impl Render for TestRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// 把菜单视图挂进窗口根节点，使菜单面板参与真实布局。
struct MenuHost {
    menu: Entity<ApplicationMenuView>,
}

impl Render for MenuHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.menu.clone())
    }
}

fn install_menus(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_menus([
            Menu::new("Zcv").items([
                MenuItem::action("关于", About),
                MenuItem::separator(),
                MenuItem::action("检查更新", CheckForUpdates),
            ]),
            Menu::new("帮助").items([
                MenuItem::action("不可用", About).disabled(true),
                MenuItem::separator(),
                MenuItem::action("检查更新", CheckForUpdates),
            ]),
        ]);
    });
}

/// 安装应用菜单并打开指定顶层菜单，返回菜单视图与窗口级上下文。
fn open_application_menu(
    cx: &mut TestAppContext,
    index: usize,
) -> (Entity<ApplicationMenuView>, &mut VisualTestContext) {
    install_menus(cx);
    let (host, visual) = cx.add_window_view(|_, cx| {
        let menu = cx.new(|cx| ApplicationMenuView::new(PopoverLayer::default(), cx));
        MenuHost { menu }
    });
    let window = *visual.windows().last().expect("窗口应已创建");
    visual.simulate_window_resize(window, size(px(720.0), px(600.0)));
    let menu = visual.read_entity(&host, |host, _| host.menu.clone());
    menu.update_in(visual, |menu, window, cx| menu.open_menu(index, window, cx));
    visual.run_until_parked();
    (menu, visual)
}

/// 读取当前展开的菜单面板。
fn expanded_panel(
    menu: &Entity<ApplicationMenuView>,
    visual: &VisualTestContext,
) -> Entity<ContextMenu> {
    visual.read_entity(menu, |menu, _| {
        menu.menu.clone().expect("菜单面板应处于展开状态")
    })
}

/// 菜单关闭后焦点归还打开前的位置。
#[gpui::test]
fn application_menu_restores_focus_after_close(cx: &mut TestAppContext) {
    install_menus(cx);
    let (_, visual) = cx.add_window_view(|_, _| TestRoot);
    let menu =
        visual.new_window_entity(|_, cx| ApplicationMenuView::new(PopoverLayer::default(), cx));
    let previous = visual.update(|_, cx| cx.focus_handle());
    visual.update(|window, cx| window.focus(&previous, cx));
    menu.update_in(visual, |menu, window, cx| menu.open_menu(0, window, cx));
    visual.run_until_parked();

    // 通过面板发出取消（与 Esc 同路径），焦点应回到打开前的位置。
    let panel = expanded_panel(&menu, visual);
    panel.update_in(visual, |panel, window, cx| {
        panel.cancel(&menu::Cancel, window, cx)
    });
    visual.run_until_parked();

    visual.update(|window, _| assert!(previous.is_focused(window)));
}

/// 悬停切换顶层菜单：面板重建为对应菜单，禁用项不可被键盘选中。
#[gpui::test]
fn application_menu_switches_on_hover(cx: &mut TestAppContext) {
    let (menu, visual) = open_application_menu(cx, 0);
    let first_panel = expanded_panel(&menu, visual);

    menu.update_in(visual, |menu, window, cx| {
        menu.hover_menu(1, true, window, cx)
    });
    visual.run_until_parked();

    menu.update_in(visual, |menu, _, _| {
        assert_eq!(menu.active_menu, 1, "悬停应切换到第二个顶层菜单");
        assert!(menu.host.is_open(), "悬停切换不应关闭浮层会话");
    });
    let second_panel = expanded_panel(&menu, visual);
    assert_ne!(
        first_panel.entity_id(),
        second_panel.entity_id(),
        "切换顶层菜单后应重建面板"
    );

    second_panel.update_in(visual, |panel, window, cx| {
        panel.select_first(&menu::SelectFirst, window, cx);
        assert_eq!(
            panel.selected_index(),
            Some(2),
            "应跳过禁用项与分隔线选中首个可用项"
        );
    });
}

/// 确认菜单项：派发 action 并关闭浮层会话。
#[gpui::test]
fn application_menu_dispatches_confirmed_action(cx: &mut TestAppContext) {
    let invoked = Rc::new(Cell::new(false));
    let invoked_by_action = invoked.clone();
    cx.update(|cx| {
        cx.on_action(move |_: &About, _| invoked_by_action.set(true));
    });

    let (menu, visual) = open_application_menu(cx, 0);
    let panel = expanded_panel(&menu, visual);
    panel.update_in(visual, |panel, window, cx| {
        panel.select_first(&menu::SelectFirst, window, cx);
        panel.confirm(&menu::Confirm, window, cx);
    });
    visual.run_until_parked();

    assert!(invoked.get(), "确认应派发菜单项 action");
    menu.update_in(visual, |menu, _, _| {
        assert!(!menu.host.is_open(), "确认后浮层会话应关闭");
        assert!(menu.menu.is_none(), "确认后面板应已清空");
    });
}
