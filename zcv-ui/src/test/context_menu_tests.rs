use std::{cell::Cell, rc::Rc};

use gpui::{
    App, Context, DismissEvent, Entity, Pixels, Render, TestAppContext, VisualTestContext, Window,
    div, px, size,
};
use zcv_actions::{About, menu};
use zcv_theme::scale;

use super::*;

/// 把菜单面板挂进窗口根节点，使菜单行的实际布局可被测量。
struct MenuHost {
    menu: Entity<ContextMenu>,
}

impl Render for MenuHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // 真实挂载（anchored 绝对定位）下面板宽度收缩到内容；
        // flex 容器在测试里提供同等的收缩上下文。
        div().size_full().flex().child(self.menu.clone())
    }
}

/// 结构尺寸是 rem 派生的分数值，布局会取整到像素；断言留半像素容差。
fn assert_close(actual: Pixels, expected: Pixels, what: &str) {
    assert!(
        (f32::from(actual) - f32::from(expected)).abs() < 0.5,
        "{what} 应在布局取整容差内：实际 {actual:?}，期望 {expected:?}"
    );
}

/// 构建菜单面板并挂进窗口，返回面板实体与窗口级上下文。
fn open_menu(
    cx: &mut TestAppContext,
    build: impl FnOnce(ContextMenu, &mut Window, &mut Context<ContextMenu>) -> ContextMenu,
) -> (Entity<ContextMenu>, &mut VisualTestContext) {
    let (host, visual) = cx.add_window_view(move |window, cx| {
        let menu = ContextMenu::build(window, cx, build);
        MenuHost { menu }
    });
    let window = *visual.windows().last().expect("窗口应已创建");
    visual.simulate_window_resize(window, size(px(720.0), px(600.0)));
    let menu = visual.read_entity(&host, |host, _| host.menu.clone());
    (menu, visual)
}

/// 确认选中项：派发菜单项的 action 并发出关闭事件供宿主关闭浮层。
#[gpui::test]
fn confirm_dispatches_action_and_emits_dismiss(cx: &mut TestAppContext) {
    let invoked = Rc::new(Cell::new(false));
    let invoked_by_action = invoked.clone();
    cx.update(|cx| {
        cx.on_action(move |_: &About, _| invoked_by_action.set(true));
    });

    let (menu, visual) = open_menu(cx, |menu, _, _| {
        menu.entry(ContextMenuEntry::action("关于", Box::new(About)))
    });

    let dismissed = Rc::new(Cell::new(false));
    let dismissed_by_event = dismissed.clone();
    visual.update(|_, cx| {
        cx.subscribe(
            &menu,
            move |_: Entity<ContextMenu>, _: &DismissEvent, _: &mut App| {
                dismissed_by_event.set(true);
            },
        )
        .detach();
    });

    menu.update_in(visual, |menu, window, cx| {
        menu.select_first(&menu::SelectFirst, window, cx);
        menu.confirm(&menu::Confirm, window, cx);
    });
    cx.run_until_parked();

    assert!(invoked.get(), "确认应派发菜单项的 action");
    assert!(dismissed.get(), "确认后应发出 DismissEvent");
}

/// 键盘导航跳过分隔线与禁用项，并在首尾环绕。
#[gpui::test]
fn navigation_skips_separators_disabled_and_wraps(cx: &mut TestAppContext) {
    let (menu, visual) = open_menu(cx, |menu, _, _| {
        menu.entry(ContextMenuEntry::new("甲"))
            .separator()
            .entry(ContextMenuEntry::new("乙").disabled(true))
            .entry(ContextMenuEntry::new("丙"))
    });

    menu.update_in(visual, |menu, window, cx| {
        menu.select_first(&menu::SelectFirst, window, cx);
        assert_eq!(menu.selected_index(), Some(0), "第一项应被选中");

        menu.select_next(&menu::SelectNext, window, cx);
        assert_eq!(menu.selected_index(), Some(3), "下一项应跳过分隔线与禁用项");

        menu.select_next(&menu::SelectNext, window, cx);
        assert_eq!(menu.selected_index(), Some(0), "末尾后应环绕回首项");

        menu.select_previous(&menu::SelectPrevious, window, cx);
        assert_eq!(menu.selected_index(), Some(3), "首项之前应环绕到末项");
    });
}

/// 取消与无选中项的确认都只关闭菜单，不执行任何条目。
#[gpui::test]
fn cancel_and_confirm_without_selection_only_dismiss(cx: &mut TestAppContext) {
    let (menu, visual) = open_menu(cx, |menu, _, _| menu.entry(ContextMenuEntry::new("甲")));

    let dismiss_count = Rc::new(Cell::new(0usize));
    let count_by_event = dismiss_count.clone();
    visual.update(|_, cx| {
        cx.subscribe(
            &menu,
            move |_: Entity<ContextMenu>, _: &DismissEvent, _: &mut App| {
                count_by_event.set(count_by_event.get() + 1)
            },
        )
        .detach();
    });

    menu.update_in(visual, |menu, window, cx| {
        menu.confirm(&menu::Confirm, window, cx);
        menu.cancel(&menu::Cancel, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(dismiss_count.get(), 2, "无选中的确认与取消都应发出关闭事件");
}

/// 菜单项名称贴行首、行尾说明贴行尾；没有说明的项不留占位，
/// 面板宽度由行内容撑开而不是固定值。
#[gpui::test]
fn rows_align_label_and_end_slot_without_reserved_blank(cx: &mut TestAppContext) {
    let (_, cx) = open_menu(cx, |menu, _, _| {
        menu.entry(ContextMenuEntry::new("保存").end_slot_text("Ctrl-S"))
            .separator()
            .entry(ContextMenuEntry::new("关于 Zcv"))
    });

    let rem_size = cx.update(|window, _| window.rem_size());
    let padding = scale::to_pixels_at(scale::S6, rem_size);
    let rows = cx
        .debug_bounds("context-menu-rows")
        .expect("菜单面板应参与布局");
    let row = cx
        .debug_bounds("context-menu-item-0")
        .expect("菜单行应参与布局");
    let label = cx
        .debug_bounds("context-menu-label-0")
        .expect("菜单名称应参与布局");
    let shortcut = cx
        .debug_bounds("context-menu-shortcut-0")
        .expect("带行尾说明的菜单项应显示说明");

    assert_close(label.left(), row.left() + padding, "名称左边缘应贴行内边距");
    assert_close(
        shortcut.right(),
        row.right() - padding,
        "行尾说明应贴住行内边距，不残留空白",
    );
    assert_close(row.size.width, rows.size.width, "菜单行应铺满面板宽度");
    assert!(
        cx.debug_bounds("context-menu-shortcut-2").is_none(),
        "没有行尾说明的菜单项不应留出空白占位"
    );
}

/// 勾选列只在菜单确实含勾选项时占用；含勾选项时同列名称保持左对齐。
#[gpui::test]
fn check_column_is_reserved_only_for_checked_items(cx: &mut TestAppContext) {
    let (_, cx) = open_menu(cx, |menu, _, _| menu.entry(ContextMenuEntry::new("撤销")));
    let rem_size = cx.update(|window, _| window.rem_size());
    let padding = scale::to_pixels_at(scale::S6, rem_size);
    let plain_row = cx
        .debug_bounds("context-menu-item-0")
        .expect("菜单行应参与布局");
    let plain_label = cx
        .debug_bounds("context-menu-label-0")
        .expect("菜单名称应参与布局");
    assert_close(
        plain_label.left(),
        plain_row.left() + padding,
        "没有可勾选项的菜单不预留勾选列",
    );

    let (_, cx) = open_menu(cx, |menu, _, _| {
        menu.entry(ContextMenuEntry::new("显示缩进参考线").checked(true))
            .entry(ContextMenuEntry::new("展开全部折叠"))
    });
    let checked_row = cx
        .debug_bounds("context-menu-item-0")
        .expect("勾选行应参与布局");
    let checked_label = cx
        .debug_bounds("context-menu-label-0")
        .expect("勾选项名称应参与布局");
    let plain_label = cx
        .debug_bounds("context-menu-label-1")
        .expect("普通项名称应参与布局");

    assert!(
        cx.debug_bounds("context-menu-check-0").is_some(),
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
