use std::{cell::Cell, rc::Rc};

use gpui::{
    IntoElement, Menu, MenuItem, Pixels, Render, TestAppContext, VisualContext, VisualTestContext,
    px, size,
};
use zcv_actions::{About, CheckForUpdates, Save, ToggleIndentGuides, Undo, UnfoldAll};

use super::*;

struct TestRoot;

impl Render for TestRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// 把菜单视图挂进窗口根节点，使菜单行的实际布局可被测量。
struct MenuHost {
    menu: Entity<ApplicationMenuView>,
}

impl Render for MenuHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.menu.clone())
    }
}

/// 结构尺寸是 rem 派生的分数值，布局会取整到像素；断言留半像素容差。
fn assert_close(actual: Pixels, expected: Pixels, what: &str) {
    assert!(
        (f32::from(actual) - f32::from(expected)).abs() < 0.5,
        "{what} 应在布局取整容差内：实际 {actual:?}，期望 {expected:?}"
    );
}

fn install_builtin_keybindings(cx: &mut TestAppContext) {
    cx.update(|cx| zcv_keymap::init(cx).expect("内置快捷键应能加载"));
}

/// 安装单个菜单并以展开态渲染，返回窗口级上下文供测量布局。
fn open_single_menu(
    cx: &mut TestAppContext,
    items: Vec<MenuItem>,
) -> (Entity<MenuHost>, &mut VisualTestContext) {
    cx.update(|cx| cx.set_menus([Menu::new("文件").items(items)]));
    let (host, visual) = cx.add_window_view(|_, cx| {
        let menu = cx.new(|cx| ApplicationMenuView::new(PopoverLayer::default(), cx));
        MenuHost { menu }
    });
    let window = *visual.windows().last().expect("窗口应已创建");
    visual.simulate_window_resize(window, size(px(720.0), px(600.0)));
    let menu = visual.read_entity(&host, |host, _| host.menu.clone());
    menu.update_in(visual, |menu, window, cx| menu.open_menu(0, window, cx));
    visual.run_until_parked();
    (host, visual)
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
    menu.update_in(visual, |menu, window, cx| {
        menu.close(&CloseMenu, window, cx)
    });
    visual.update(|window, _| assert!(previous.is_focused(window)));
}

#[gpui::test]
fn application_menu_switches_on_hover_and_ignores_disabled_items(cx: &mut TestAppContext) {
    install_menus(cx);
    let (_, visual) = cx.add_window_view(|_, _| TestRoot);
    let menu =
        visual.new_window_entity(|_, cx| ApplicationMenuView::new(PopoverLayer::default(), cx));
    menu.update_in(visual, |menu, window, cx| {
        menu.open_menu(0, window, cx);
        menu.hover_menu(1, true, cx);
        assert_eq!(menu.active_menu, 1);
        menu.activate(0, window, cx);
        assert!(menu.host.is_open());
        menu.hover_menu(0, true, cx);
        assert_eq!(menu.active_menu, 0);
    });
}

#[gpui::test]
fn application_menu_dispatches_clicked_action(cx: &mut TestAppContext) {
    let invoked = Rc::new(Cell::new(false));
    let invoked_by_action = invoked.clone();
    cx.update(|cx| {
        cx.on_action(move |_: &CheckForUpdates, _| invoked_by_action.set(true));
    });
    install_menus(cx);
    let (_, visual) = cx.add_window_view(|_, _| TestRoot);
    let menu =
        visual.new_window_entity(|_, cx| ApplicationMenuView::new(PopoverLayer::default(), cx));
    menu.update_in(visual, |menu, window, cx| {
        menu.open_menu(0, window, cx);
        menu.hover_menu(1, true, cx);
        menu.activate(2, window, cx);
    });
    visual.run_until_parked();
    assert!(invoked.get());
}

/// 菜单项名称贴行首、快捷键贴行尾；没有快捷键的项不留占位，
/// 面板宽度由行内容撑开而不是固定值。
#[gpui::test]
fn menu_rows_align_shortcut_to_row_end_without_reserved_blank(cx: &mut TestAppContext) {
    install_builtin_keybindings(cx);
    let (_, cx) = open_single_menu(
        cx,
        vec![
            MenuItem::action("保存", Save),
            MenuItem::separator(),
            MenuItem::action("关于 Zcv", About),
        ],
    );

    let rem_size = cx.update(|window, _| window.rem_size());
    let padding = scale::to_pixels_at(scale::S6, rem_size);
    let rows = cx
        .debug_bounds("application-menu-rows")
        .expect("菜单面板应参与布局");
    let row = cx
        .debug_bounds("application-menu-item-0")
        .expect("菜单行应参与布局");
    let label = cx
        .debug_bounds("application-menu-label-0")
        .expect("菜单名称应参与布局");
    let shortcut = cx
        .debug_bounds("application-menu-shortcut-0")
        .expect("已绑定快捷键的菜单项应显示快捷键");

    assert_close(
        label.left(),
        row.left() + padding,
        "名称左边缘：没有可勾选项的菜单不预留勾选列",
    );
    assert_close(
        shortcut.right(),
        row.right() - padding,
        "快捷键右边缘应贴住行内边距，不残留空白",
    );
    assert_close(row.size.width, rows.size.width, "菜单行应铺满面板宽度");
    assert!(
        cx.debug_bounds("application-menu-shortcut-2").is_none(),
        "没有快捷键的菜单项不应留出空白占位"
    );
}

/// 面板宽度随菜单内容变化：短标签菜单不再被固定宽度撑出右侧空白。
#[gpui::test]
fn menu_panel_width_follows_row_content(cx: &mut TestAppContext) {
    install_builtin_keybindings(cx);

    let (_, cx) = open_single_menu(cx, vec![MenuItem::action("撤销", Undo)]);
    let short = cx
        .debug_bounds("application-menu-rows")
        .expect("短标签菜单应参与布局")
        .size
        .width;

    let (_, cx) = open_single_menu(
        cx,
        vec![MenuItem::action("切换到上一个标签页的完整标题", Undo)],
    );
    let long = cx
        .debug_bounds("application-menu-rows")
        .expect("长标签菜单应参与布局")
        .size
        .width;

    assert!(
        short < px(220.0),
        "短标签菜单的宽度应由内容决定，不应被固定宽度撑出空白：实际 {short:?}"
    );
    assert!(
        long > short,
        "面板宽度应随菜单内容增长：短标签 {short:?}，长标签 {long:?}"
    );
}

/// 勾选列只在菜单确实含勾选项时占用；含勾选项时同列名称保持左对齐。
#[gpui::test]
fn check_column_is_reserved_only_for_menus_with_checked_items(cx: &mut TestAppContext) {
    install_builtin_keybindings(cx);

    let (_, cx) = open_single_menu(cx, vec![MenuItem::action("撤销", Undo)]);
    let rem_size = cx.update(|window, _| window.rem_size());
    let padding = scale::to_pixels_at(scale::S6, rem_size);
    let plain_row = cx
        .debug_bounds("application-menu-item-0")
        .expect("菜单行应参与布局");
    let plain_label = cx
        .debug_bounds("application-menu-label-0")
        .expect("菜单名称应参与布局");
    assert_close(
        plain_label.left(),
        plain_row.left() + padding,
        "没有可勾选项的菜单不预留勾选列",
    );

    let (_, cx) = open_single_menu(
        cx,
        vec![
            MenuItem::action("显示缩进参考线", ToggleIndentGuides).checked(true),
            MenuItem::action("展开全部折叠", UnfoldAll),
        ],
    );
    let checked_row = cx
        .debug_bounds("application-menu-item-0")
        .expect("勾选行应参与布局");
    let checked_label = cx
        .debug_bounds("application-menu-label-0")
        .expect("勾选项名称应参与布局");
    let plain_label = cx
        .debug_bounds("application-menu-label-1")
        .expect("普通项名称应参与布局");

    assert!(
        cx.debug_bounds("application-menu-check-0").is_some(),
        "勾选项应显示勾选图标"
    );
    assert_close(
        checked_label.left(),
        plain_label.left(),
        "同一菜单内的名称应左对齐",
    );
    assert!(
        checked_label.left() > checked_row.left() + padding,
        "含勾选项的菜单应为勾选列留出缩进：名称 {:?}，行 {:?}",
        checked_label.left(),
        checked_row.left(),
    );
}
