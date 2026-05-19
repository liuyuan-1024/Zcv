//! 文件树焦点与显示策略。
//!
//! 文件树是否显示仍由 WorkbenchController 维护；本模块集中保存文件树面板
//! 自己的焦点语义，避免散落在 ShellView 里。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Context, FocusHandle, Window};

use crate::app::App;
use crate::shell::features::PanelId;
use crate::shell::workbench::controller::WorkbenchController;

/// 注册文件树焦点监听：获焦时初始化首个可见行，两端都刷新高亮状态。
pub(crate) fn install_focus_listeners<T: 'static>(
    app: Rc<RefCell<App>>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    cx.on_focus(focus, window, move |_, _, cx| {
        app.borrow_mut().file_tree_ensure_selection_initialized();
        cx.notify();
    })
    .detach();
    cx.on_blur(focus, window, |_, _, cx| {
        cx.notify();
    })
    .detach();
}

/// 文件树入口请求：只处理文件树面板自己的显隐与焦点切换。
pub(crate) fn handle_toggle_request(
    workbench: &Rc<RefCell<WorkbenchController>>,
    editor_focus_fallback: &FocusHandle,
    file_tree_focus: &FocusHandle,
    window: &mut Window,
) {
    apply_toggle(workbench, editor_focus_fallback, file_tree_focus, window);
    window.refresh();
}

/// 文件树切换三态：
///
/// - 当前可见且面板有焦点：收起，焦点回到 editor。
/// - 当前可见但焦点在别处：不收起，只把焦点切到文件树。
/// - 当前不可见：展开并聚焦。
fn apply_toggle(
    workbench: &Rc<RefCell<WorkbenchController>>,
    editor_focus_fallback: &FocusHandle,
    file_tree_focus: &FocusHandle,
    window: &mut Window,
) {
    let visible = workbench.borrow().is_panel_active(PanelId::FileTree);
    let has_focus = file_tree_focus.is_focused(window);
    match (visible, has_focus) {
        (true, true) => {
            workbench.borrow_mut().toggle_panel(PanelId::FileTree);
            window.focus(editor_focus_fallback);
        }
        (true, false) => {
            window.focus(file_tree_focus);
        }
        (false, _) => {
            workbench.borrow_mut().toggle_panel(PanelId::FileTree);
            window.focus(file_tree_focus);
        }
    }
}

/// 打开项目后调用：确保文件树面板可见且为 active panel，然后把焦点交给它。
pub(crate) fn reveal_and_focus(
    workbench: &Rc<RefCell<WorkbenchController>>,
    file_tree_focus: &FocusHandle,
    window: &mut Window,
) {
    if !workbench.borrow().is_panel_active(PanelId::FileTree) {
        workbench.borrow_mut().toggle_panel(PanelId::FileTree);
    }
    window.focus(file_tree_focus);
    let file_tree_focus = file_tree_focus.clone();
    window.on_next_frame(move |window, _| {
        window.focus(&file_tree_focus);
    });
}
