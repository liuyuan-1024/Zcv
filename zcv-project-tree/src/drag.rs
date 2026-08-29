//! 树内拖拽移动：拖拽载荷、跟随预览、落点目标解析与合法性过滤。
//!
//! 拖拽移动与剪切粘贴共用同一条 Move + 冲突确认管线（drop 最终调用 `ProjectTreePanel::begin_transfer`，见 mod.rs），本模块只承载与面板状态无关的纯逻辑，便于单测覆盖。

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{App, Context, Div, FontWeight, Window, div, prelude::*, px};
use zcv_theme::{FileIcons, color, space, typography};
use zcv_ui::SvgIcon;

use super::transfer::paste_target_dir;

/// 树内拖拽载荷：被拖行与渲染期冻结的多选标记快照，跟随预览显示的名称。
///
/// 载荷在渲染期随元素构造冻结，拖拽发起与放下都信任这份快照（gpui 的 on_drag 取的是最后一次渲染写入元素状态的值），因此拖拽内容恒等于用户发起拖拽时所见选区，不受拖拽期间任何状态变化影响。
#[derive(Clone)]
pub(crate) struct TreeDrag {
    /// 被拖行路径。
    pub(crate) active_selection: PathBuf,
    /// 渲染期快照的多选标记集（可见行序）。
    pub(crate) marked_selections: Rc<[PathBuf]>,
    /// 跟随预览显示的条目名（被拖行的名称）。
    pub(crate) preview_name: String,
}

impl TreeDrag {
    /// 参与本次拖拽的源路径：被拖行在多选标记集内 → 整个标记集；否则仅被拖行。
    pub(crate) fn items(&self) -> Vec<PathBuf> {
        if self.marked_selections.contains(&self.active_selection) {
            self.marked_selections.to_vec()
        } else {
            vec![self.active_selection.clone()]
        }
    }
}

/// 拖拽跟随预览视图（拖拽幽灵）：单项为单行（图标 + 名称）；
/// 多选为堆叠卡片——顶层被拖行附醒目数量徽标，其后最多两层后景卡片（真实选中项，逐层右下偏移），使选区拖影与单项拖影一眼可辨。
///
/// 由行元素 `on_drag` 构造为 Entity 注册，拖拽进行时由 gpui 拖拽系统跟随光标渲染。
pub(crate) struct DraggedEntryView {
    drag: TreeDrag,
}

/// 顶层之外的后景卡片层数：堆叠共 3 层，精确数量由徽标传达。
const STACK_BACK_LAYERS: usize = 6;
/// 后景卡片逐层向右下偏移的步长（px）：步长要足够露出堆叠轮廓，与单项拖影一眼可辨。
const LAYER_OFFSET_X: f32 = 6.;
const LAYER_OFFSET_Y: f32 = 8.;

impl DraggedEntryView {
    pub(crate) fn new(drag: TreeDrag) -> Self {
        Self { drag }
    }
}

impl Render for DraggedEntryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *color::current(cx);
        let items = self.drag.items();
        let count = items.len();
        if count == 1 {
            // 单项拖影：单行图标 + 名称，无徽标，保持简洁。
            return entry_card(cx)
                .child(SvgIcon::new(entry_icon(&self.drag.active_selection)).size(typography::ui()))
                .child(self.drag.preview_name.clone());
        }
        // 多选堆叠卡片：后景层绝对定位逐层右下偏移、先挂载（越远越先），顶层卡片常流最后挂载；
        // gpui 子元素按序绘制，保证顶层在最上且撑起容器尺寸；
        // 后景层越界溢出容器是预期效果（拖拽幽灵不裁剪）。
        let active = &self.drag.active_selection;
        let mut stack = div().relative();
        // 后景层取选区内除被拖行外的前几项（被拖行在快照中的位置不一定居首）。
        let back_layers: Vec<_> = items
            .iter()
            .filter(|path| *path != active)
            .take(STACK_BACK_LAYERS)
            .collect();
        let layer_total = back_layers.len();
        for (i, path) in back_layers.into_iter().rev().enumerate() {
            let depth = (layer_total - i) as f32;
            stack = stack.child(
                entry_card(cx)
                    .border_1()
                    .border_color(theme.border_variant)
                    .absolute()
                    .top(px(LAYER_OFFSET_Y * depth))
                    .left(px(LAYER_OFFSET_X * depth))
                    .text_color(theme.text_muted)
                    .child(SvgIcon::new(entry_icon(path)).size(typography::ui()))
                    .child(entry_name(path)),
            );
        }
        stack.child(
            entry_card(cx)
                .border_1()
                .border_color(theme.border_variant)
                .shadow_sm()
                .child(SvgIcon::new(entry_icon(active)).size(typography::ui()))
                .child(self.drag.preview_name.clone())
                // 数量徽标：反色实心圆角胶囊（正文色底 + 底色字），任何主题下都高对比醒目。
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(16.))
                        .min_w(px(16.))
                        .px(space::S2)
                        .rounded_full()
                        .bg(theme.text)
                        .text_color(theme.background)
                        .font_weight(FontWeight::BOLD)
                        .child(format!("{count} 项")),
                ),
        )
    }
}

/// 幽灵卡片基调：与面板行同源（一行高、选中色底、图标 + 名称）。
fn entry_card(cx: &App) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::S6)
        .px(space::S6)
        .h(typography::ui_line())
        .rounded_xs()
        .bg(color::current(cx).element_selected)
}

/// 选中条目的图标：目录/文件按本地 metadata 实时判断（后景层每帧至多 2 次，开销可忽略）。
fn entry_icon(path: &Path) -> String {
    if path.is_dir() {
        FileIcons::get_folder_icon(false, path)
    } else {
        FileIcons::get_icon(path)
    }
}

/// 后景卡片显示的条目名：取路径文件名。
fn entry_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 落点目标目录解析（纯函数）：目录行（含根行）→自身路径；文件行→其父目录。
///
/// 根行即项目根目录行（`is_dir` 为真且路径等于根），命中「目录行→自身」规则；
/// 与粘贴目标推断复用同一实现，避免两处语义漂移。
pub(crate) fn drop_target_dir(row_path: &Path, row_is_dir: bool) -> Option<PathBuf> {
    paste_target_dir(Some(row_path), row_is_dir)
}

/// 合法性过滤（纯函数）：逐项判断拖拽源能否移入目标目录，返回可移动的源列表。
///
/// 拒绝规则：
/// - 目标落在源自身子树内（含源本身）：`target_dir.starts_with(source)`，目录不能移入自己的后代（会产生路径循环）；
/// - 移动结果与源相同（落回原目录，无变化）或目标是源的祖先目录（覆盖移动会摧毁源数据，与 `move_path` 的对称守卫一致）：静默剔除。
///
/// 多选拖拽时对每个源逐项独立判断，非法项剔除、合法项照常移动。
pub(crate) fn filter_movable_sources(sources: &[PathBuf], target_dir: &Path) -> Vec<PathBuf> {
    sources
        .iter()
        .filter(|source| {
            if target_dir.starts_with(source.as_path()) {
                return false;
            }
            match source.file_name() {
                Some(name) => {
                    let destination = target_dir.join(name);
                    // starts_with 含等号，一次判断覆盖两种情形：
                    // 目标 == 源（落回原目录无变化）与目标是源的祖先（非法落点不高亮、不进冲突浮层）。
                    !source.starts_with(&destination)
                }
                // 无文件名的路径（如项目根）不参与移动。
                None => false,
            }
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_follows_active_membership_in_marked_snapshot() {
        let root = Path::new("/proj");
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        // 被拖行在标记集内：整个标记集参与拖拽。
        let drag = TreeDrag {
            active_selection: a.clone(),
            marked_selections: vec![a.clone(), b.clone()].into(),
            preview_name: "a.txt".into(),
        };
        assert_eq!(drag.items(), vec![a.clone(), b.clone()]);
        // 被拖行不在标记集内（集合外行拖起）：仅被拖行参与。
        let drag = TreeDrag {
            active_selection: a.clone(),
            marked_selections: vec![b.clone()].into(),
            preview_name: "a.txt".into(),
        };
        assert_eq!(drag.items(), vec![a]);
    }

    #[test]
    fn drop_target_of_directory_row_is_itself() {
        let root = Path::new("/proj");
        // 目录行 → 自身路径。
        assert_eq!(
            drop_target_dir(&root.join("src"), true),
            Some(root.join("src"))
        );
        // 根行（项目根目录行）→ 项目根。
        assert_eq!(drop_target_dir(root, true), Some(root.to_path_buf()));
    }

    #[test]
    fn drop_target_of_file_row_is_its_parent_directory() {
        let root = Path::new("/proj");
        // 普通文件行 → 父目录。
        assert_eq!(
            drop_target_dir(&root.join("src").join("main.rs"), false),
            Some(root.join("src"))
        );
        // 根级文件行 → 项目根。
        assert_eq!(
            drop_target_dir(&root.join("a.txt"), false),
            Some(root.to_path_buf())
        );
    }

    #[test]
    fn moving_directory_into_own_subtree_is_rejected() {
        let root = Path::new("/proj");
        let src = root.join("src");
        let sub = src.join("sub");
        // 目标是源的后代：拒绝。
        assert_eq!(
            filter_movable_sources(std::slice::from_ref(&src), &sub),
            Vec::<PathBuf>::new()
        );
        // 目标是源本身（目录拖回自己身上）：拒绝。
        assert_eq!(
            filter_movable_sources(std::slice::from_ref(&src), &src),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn moving_into_own_ancestor_directory_is_rejected() {
        let root = Path::new("/proj");
        // 移动结果由「落点目录 + 源的最后组件名」拼出：源 /proj/a/b/a/c/a 拖到 /proj/a/b 上，
        // 结果 /proj/a/b/a 恰好是源的祖先目录，覆盖移动会摧毁源数据，必须拒绝（与 move_path 的对称守卫一致）。
        let source = root.join("a").join("b").join("a").join("c").join("a");
        let ancestor_target = root.join("a").join("b");
        assert_eq!(
            filter_movable_sources(std::slice::from_ref(&source), &ancestor_target),
            Vec::<PathBuf>::new()
        );
        // 落点是无关的兄弟目录时照常放行。
        let sibling = root.join("other");
        assert_eq!(
            filter_movable_sources(std::slice::from_ref(&source), &sibling),
            vec![source]
        );
    }

    #[test]
    fn moving_into_same_directory_is_filtered_as_noop() {
        let root = Path::new("/proj");
        let file = root.join("a.txt");
        // 落回源所在目录：目标与源相同，剔除。
        assert_eq!(
            filter_movable_sources(std::slice::from_ref(&file), root),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn moving_into_sibling_directory_is_allowed() {
        let root = Path::new("/proj");
        let file = root.join("a.txt");
        let dst = root.join("dst");
        assert_eq!(
            filter_movable_sources(std::slice::from_ref(&file), &dst),
            vec![file]
        );
    }

    #[test]
    fn multi_select_drag_filters_each_source_independently() {
        let root = Path::new("/proj");
        let src = root.join("src");
        let inner = src.join("inner.txt");
        let other = root.join("other.txt");
        // 多选拖到 src/自身：目录源因「移入自身子树」被拒，
        // src 内文件因「落回原目录」被剔除，项目外文件正常放行。
        let kept = filter_movable_sources(&[src.clone(), inner, other.clone()], &src);
        assert_eq!(kept, vec![other]);
    }

    #[test]
    fn project_root_row_is_not_movable() {
        let root = Path::new("/proj");
        // 根路径无文件名，视为不可移动。
        assert_eq!(
            filter_movable_sources(&[root.to_path_buf()], &root.join("dst")),
            Vec::<PathBuf>::new()
        );
    }
}
