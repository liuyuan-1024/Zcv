//! TopBar —— 窗口级顶部外壳（布局模型 4.2）。
//!
//! 当前固定槽：
//! - leading：窗口控制圆点 + 项目入口
//! - trailing：设置入口
//!
//! 与 BottomBar 共用 `bar_frame`，确保对称（布局模型 4.1）。

#[cfg(target_os = "linux")]
use gpui::MouseButton;
#[cfg(target_os = "windows")]
use gpui::WindowControlArea;
use gpui::{AnyElement, Div, Window, div, prelude::*};

use crate::shell::features::{project_picker, settings};
use crate::shell::shared::{CommandBinding, Glyph};
use crate::shell::surfaces::{SurfaceStates, track_surface_anchor};
use crate::shell::workbench::WorkbenchCommandRequests;
use crate::shell::workbench::state::WorkbenchState;
use crate::theme;
use crate::ui_id::SurfaceId;

use super::frame::{BarEdge, bar_frame};
use super::window_controls::{WindowControlsHandlers, render_window_controls};

pub(crate) fn render(
    state: &WorkbenchState,
    window: &Window,
    window_controls: WindowControlsHandlers,
    commands: &WorkbenchCommandRequests,
    surfaces: &SurfaceStates,
) -> Div {
    let is_window_active = window.is_window_active();

    // 结构：leading_cluster | drag_spacer | trailing_cluster
    //
    // 不挂 `on_mouse_down` 到整条 bar：GPUI 0.2.2 的 window-control hitbox 表按绘制顺序首个命中即返回，
    // 父节点标 Drag 会盖过所有子按钮（[events.rs:874] / [window.rs:1138]）。
    // 必须让 drag 区域与按钮区域是兄弟、不要嵌套。
    bar_frame(BarEdge::Top)
        .child(cluster(leading_slots(
            is_window_active,
            window_controls,
            &state.project_title,
            &state.project_branch,
            commands,
            surfaces,
        )))
        .child(drag_spacer())
        .child(cluster(trailing_slots(commands, surfaces)))
}

/// 顶栏内的按钮簇：内容自适应宽度，作为兄弟节点与 `drag_spacer` 共存。
fn cluster(items: Vec<AnyElement>) -> Div {
    div().flex().items_center().gap_2().children(items)
}

/// 顶栏中央填充区，同时承担拖动窗口的职责。
///
/// - macOS：透明系统标题栏自带拖动语义，这里只是 flex 间隔，不需任何介入。
/// - Windows：在 `WM_NCHITTEST` 里命中 `WindowControlArea::Drag` 的 hitbox 会被映射为 HTCAPTION，由 OS 接管拖动；
/// `start_window_move` 在 Windows 上是空实现，必须走这条路径。
/// - Linux (X11 / Wayland)：`window_control_area` 在这两个后端是 no-op，需要显式调用 `start_window_move`。
///
/// 高度上：bar_frame 是 `items_center` 的 flex 容器，自身高度由其它兄弟（按钮簇）撑开；
/// 这里 `h_full` 让 spacer 与 bar 内容区同高，保证 hit-test 能命中。
fn drag_spacer() -> Div {
    let spacer = div().flex_1().h_full();

    #[cfg(target_os = "windows")]
    let spacer = spacer.window_control_area(WindowControlArea::Drag);

    #[cfg(target_os = "linux")]
    let spacer = spacer.on_mouse_down(MouseButton::Left, |_, window, _| {
        window.start_window_move();
    });

    spacer
}

fn leading_slots(
    is_window_active: bool,
    window_controls: WindowControlsHandlers,
    project_title: &str,
    project_branch: &Option<String>,
    commands: &WorkbenchCommandRequests,
    surfaces: &SurfaceStates,
) -> Vec<AnyElement> {
    let workspace = project_picker::entry(
        project_title,
        surfaces.is_active(SurfaceId::ProjectPicker),
        commands.project_picker_open.clone(),
    );

    let mut slots = vec![
        render_window_controls(is_window_active, window_controls).into_any_element(),
        workspace,
    ];

    if let Some(branch) = project_branch {
        slots.push(branch_badge(
            branch,
            surfaces.is_active(SurfaceId::BranchPicker),
            commands.branch_picker_open.clone(),
        ));
    }

    slots
}

/// 顶栏分支徽章：分支图标 + 分支名。点击弹出分支选择器。
const BRANCH_BADGE_ID: &str = "top-bar.branch-badge";

fn branch_badge(branch: &str, active: bool, command: CommandBinding) -> AnyElement {
    let entry_color = if active {
        theme::color::glyph_active()
    } else {
        theme::color::glyph_default()
    };

    let glyph = Glyph::icon_text(BRANCH_BADGE_ID, "icons/panels/version_control.svg", branch)
        .color(entry_color)
        .command(command)
        .render();

    let element_id: gpui::ElementId = BRANCH_BADGE_ID.into();
    track_surface_anchor(element_id, glyph).into_any_element()
}

fn trailing_slots(
    commands: &WorkbenchCommandRequests,
    surfaces: &SurfaceStates,
) -> Vec<AnyElement> {
    vec![settings::entry(
        surfaces.is_active(SurfaceId::Settings),
        commands.settings_open.clone(),
    )]
}
