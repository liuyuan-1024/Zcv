//! 编辑器渲染快照类型。

use zom_engine::SelectionSet;
use zom_workspace::view::{RevealKind, VisualPosition};

use crate::editor::highlight::Decoration;

/// 渲染快照里一条逻辑行的描述。
///
/// `line_index` 是该行在整 buffer 范围内的 0-based 行号；`start_byte` 是该行
/// 在 buffer 中的绝对字节起点；`text` 是该行的可见文本（不含行尾换行符）。
#[derive(Clone, Debug)]
pub(crate) struct SnapshotLine {
    pub(crate) line_index: u64,
    pub(crate) start_byte: usize,
    pub(crate) text: String,
}

/// 编辑器的 owned 渲染快照。
///
/// `lines` 是当前视口切出的逻辑行集合（绝对 line/byte 坐标），由
/// `Buffer::slice_viewport` 喂入；调用方再也不读整份 buffer 文本。
#[derive(Clone, Debug, Default)]
pub(crate) struct EditorSnapshot {
    /// 视口内可见的逻辑行。空表示「buffer 无任何行可读」。
    pub(crate) lines: Vec<SnapshotLine>,
    /// buffer 总行数。gutter 据此决定行号列宽度，避免滚动时列宽抖动。
    pub(crate) total_lines: u64,
    /// view 已落定的视口顶行（0-based）；
    /// 与 snapshot 切片起点的区别在于：
    /// snapshot 切片范围带有 ±visible_lines 的安全余量，`top_line` 才是用户真正看到的第一行。
    pub(crate) top_line: u64,
    /// `top_line` 内的软换行视觉段序号（0-based）。不开软换行时为 0。
    pub(crate) top_subrow: u64,
    /// primary head 的绝对字节位（与 `selection.primary().head()` 等价）。
    pub(crate) cursor_byte: usize,
    /// primary head 的 (行号, 列号)，均 0-based；列按 Unicode scalar value 计。
    /// 底栏行 / 列展示直接读它，无需扫描文本。
    pub(crate) cursor_position: (u64, u64),
    /// 完整选区集合。
    /// `cursor_byte` 与每个 selection.head 的 caret 几何都从这里派生；
    /// 非空 selection 的 range 已作为 `Background` Decoration 加进 [`decorations`]，element 不再二次读 selection 算背景。
    pub(crate) selection: SelectionSet,
    /// primary caret 的视觉投影。
    ///
    /// Selection 只保存 byte；软换行边界处同一个 byte 可能同时是上一视觉行行尾和下一视觉行行首。
    /// 主编辑区从 View 带入这个 [`VisualPosition`]，element 据此把 primary caret 画回垂直移动命令选择的那条视觉行。
    pub(crate) visual_caret: Option<VisualPosition>,
    /// 外部 reveal 请求在快照里的表示。是否生效由 `EditorKernel` 决定。
    pub(crate) reveal: Option<RevealHint>,
    /// 上屏装饰集合——syntax / selection / search 等 producer 投递给 composer
    /// 的统一形态（手册《桌面端高亮架构》§四）。
    ///
    /// - 每个 producer 自身保证内部 range 不重叠；跨 producer 允许 Background 重叠。
    /// - 顺序无要求；shell prepaint 阶段按 [`DecorationKind`] 切分、按
    /// `priority` 排序、解析 [`StyleClass`]。
    /// - 空 Vec = 无任何装饰（plain 文本、无选区、无搜索、无 syntax）。
    pub(crate) decorations: Vec<Decoration>,
}

/// [`zom_workspace::view::RevealRequest`] 的渲染端镜像 —— 把 `ByteOffset` 换成 `usize`，
/// 便于元素侧直接用，不再跨 crate 引 engine 类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RevealHint {
    pub(crate) byte: usize,
    pub(crate) line: u64,
    pub(crate) kind: RevealKind,
    pub(crate) seq: u64,
}
