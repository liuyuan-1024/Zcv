use std::{cell::Cell, rc::Rc};

use gpui::{
    KeyBinding, KeyUpEvent, Keystroke, Modifiers, Pixels, Render, TestAppContext,
    VisualTestContext, point, px, size,
};
use zcv_actions::menu;

use super::*;

struct DropdownExample {
    layer: PopoverLayer,
    chosen: Rc<Cell<usize>>,
    trigger_y: Pixels,
}

impl Render for DropdownExample {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let labels = ["竖线", "实心方块", "下划线", "空心方块"];
        let current = self.chosen.get();
        let entries = labels
            .into_iter()
            .enumerate()
            .map(|(index, label)| {
                let chosen = self.chosen.clone();
                ContextMenuEntry::new(label)
                    .checked(index == current)
                    .handler(move |_, _, _| chosen.set(index))
            })
            .collect();
        div()
            .size_full()
            .p(scale::S16)
            .pt(self.trigger_y)
            .child(DropdownMenu::new(
                "cursor",
                labels[current],
                entries,
                self.layer.clone(),
            ))
    }
}

fn press_enter(cx: &mut VisualTestContext) {
    cx.simulate_keystrokes("enter");
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").expect("Enter 应能解析"),
    });
}

#[gpui::test]
fn trigger_width_follows_the_current_label(cx: &mut TestAppContext) {
    let (example, visual) = cx.add_window_view(|_, _| DropdownExample {
        layer: PopoverLayer::default(),
        chosen: Rc::new(Cell::new(0)),
        trigger_y: px(16.0),
    });
    let width = |visual: &mut VisualTestContext| {
        visual.debug_bounds("dropdown-trigger").unwrap().size.width
    };
    let short = width(visual);

    example.update(visual, |example, cx| {
        example.chosen.set(1);
        cx.notify();
    });

    assert!(width(visual) > short, "较长的当前值应撑宽按钮");
}

#[gpui::test]
fn navigation_starts_at_current_value_and_only_confirmation_commits(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.bind_keys([
            KeyBinding::new("down", menu::SelectNext, Some("menu")),
            KeyBinding::new("enter", menu::Confirm, Some("menu")),
            KeyBinding::new("escape", menu::Cancel, Some("menu")),
        ]);
    });
    let chosen = Rc::new(Cell::new(1));
    let (_, visual) = cx.add_window_view(|_, _| DropdownExample {
        layer: PopoverLayer::default(),
        chosen: chosen.clone(),
        trigger_y: px(16.0),
    });
    let window = *visual.windows().last().expect("窗口应已创建");
    visual.simulate_window_resize(window, size(px(720.0), px(600.0)));
    let trigger = visual
        .debug_bounds("dropdown-trigger")
        .expect("下拉按钮应可见");
    visual.simulate_click(trigger.center(), Modifiers::default());
    let panel = visual
        .debug_bounds("overlay-panel")
        .expect("点击应打开菜单");
    assert!(
        panel.top() >= trigger.bottom() - px(1.0),
        "菜单应锚定在按钮下方"
    );

    visual.simulate_keystrokes("down");
    assert_eq!(chosen.get(), 1, "移动高亮不应提交新值");
    visual.simulate_keystrokes("escape");
    assert!(
        visual.debug_bounds("overlay-panel").is_none(),
        "Esc 应关闭菜单"
    );
    assert_eq!(chosen.get(), 1, "关闭菜单不应提交新值");

    press_enter(visual);
    assert!(
        visual.debug_bounds("overlay-panel").is_some(),
        "焦点应返回按钮，可按 Enter 打开"
    );
    visual.simulate_keystrokes("down enter");
    assert_eq!(chosen.get(), 2, "键盘应从当前值开始导航，确认后提交下一项");
    assert!(
        visual.debug_bounds("overlay-panel").is_none(),
        "确认应关闭菜单"
    );

    press_enter(visual);
    assert!(
        visual.debug_bounds("overlay-panel").is_some(),
        "应能再次打开菜单"
    );
    visual.simulate_click(point(px(600.0), px(500.0)), Modifiers::default());
    assert!(
        visual.debug_bounds("overlay-panel").is_none(),
        "点击外部应关闭菜单"
    );
    assert_eq!(chosen.get(), 2, "点击外部不应提交新值");

    visual.simulate_click(trigger.center(), Modifiers::default());
    let row = visual
        .debug_bounds("context-menu-item-3")
        .expect("展开后应显示第四个选项");
    visual.simulate_click(row.center(), Modifiers::default());
    assert_eq!(chosen.get(), 3, "点击条目应提交该选项");
    assert!(
        visual.debug_bounds("overlay-panel").is_none(),
        "选择后应关闭菜单"
    );
}

#[gpui::test]
fn menu_opens_above_a_trigger_near_the_bottom(cx: &mut TestAppContext) {
    let (_, visual) = cx.add_window_view(|_, _| DropdownExample {
        layer: PopoverLayer::default(),
        chosen: Rc::new(Cell::new(0)),
        trigger_y: px(240.0),
    });
    let window = *visual.windows().last().expect("窗口应已创建");
    visual.simulate_window_resize(window, size(px(320.0), px(300.0)));
    let trigger = visual
        .debug_bounds("dropdown-trigger")
        .expect("下拉按钮应可见");
    visual.simulate_click(trigger.center(), Modifiers::default());
    let panel = visual
        .debug_bounds("overlay-panel")
        .expect("点击应打开菜单");
    assert!(
        panel.bottom() < trigger.top(),
        "下方空间不足时，菜单应在按钮上方展开"
    );
    assert!(panel.top() > px(0.0), "菜单应保持在窗口内");
    assert_eq!(panel.left(), trigger.left(), "菜单应与按钮左对齐");
}

#[gpui::test]
fn menu_fits_without_covering_the_trigger_in_a_short_window(cx: &mut TestAppContext) {
    let (_, visual) = cx.add_window_view(|_, _| DropdownExample {
        layer: PopoverLayer::default(),
        chosen: Rc::new(Cell::new(0)),
        trigger_y: px(48.0),
    });
    let window = *visual.windows().last().expect("窗口应已创建");
    visual.simulate_window_resize(window, size(px(320.0), px(145.0)));
    let trigger = visual
        .debug_bounds("dropdown-trigger")
        .expect("下拉按钮应可见");
    visual.simulate_click(trigger.center(), Modifiers::default());
    let panel = visual
        .debug_bounds("overlay-panel")
        .expect("点击应打开菜单");
    let rows = visual
        .debug_bounds("context-menu-rows")
        .expect("菜单选项应可见");
    assert!(
        panel.top() > trigger.bottom(),
        "菜单应优先使用更宽裕的下方空间"
    );
    assert!(panel.bottom() < px(145.0), "菜单面板应收缩到窗口内");
    assert!(
        rows.bottom() < panel.bottom(),
        "可滚动的选项区域应留在菜单边框内"
    );
}
