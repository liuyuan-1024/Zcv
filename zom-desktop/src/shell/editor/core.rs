//! 可嵌入编辑器本体：buffer、selection 与命令作用目标。

use zom_command::EditTarget;
use zom_engine::{Buffer, BufferConfig, Line, SelectionSet, TextRange, Viewport};
use zom_view::RevealKind;

use super::ime::{ImeQueryTarget, ImeTarget};

/// 一个独立的文本编辑单元：自持 buffer 与选区。
pub(crate) struct Editor {
    buffer: Buffer,
    /// 视图侧的权威选区；编辑命令读它、写回它（与主编辑区 view/buffer
    /// 双选区模型一致）。
    selection: SelectionSet,
}

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
/// `Buffer::slice_viewport` 喂入；调用方再也不读整份 buffer 文本。`total_lines`
/// 让渲染端按总行数算 content_height，从而不依赖 `lines.len()` 做 scroll clamp。
///
/// `cursor_byte` 与 `selection` 是平行字段：前者是 primary head 的纯 byte 投影
/// （供 blink / 状态栏 / 测试这些"只关心活动光标在哪"的下游消费），
/// 后者是完整 SelectionSet（供 [`super::element::EditorElement`] 渲染多光标 caret + 选区背景）。
/// 两个字段必须从同一份 SelectionSet 派生、不允许漂移：构造快照的
/// 唯一入口在 [`Editor::snapshot`] 与 [`super::main_editor`]，那里保证一致。
#[derive(Clone, Debug, Default)]
pub(crate) struct EditorSnapshot {
    /// 视口内可见的逻辑行。空表示「buffer 无任何行可读」。
    pub(crate) lines: Vec<SnapshotLine>,
    /// 整个 buffer 的逻辑行总数。渲染端按它算 `content_height`。
    pub(crate) total_lines: u64,
    /// `lines` 的第一条对应的逻辑行号（0-based）；空 lines 时为 0。
    pub(crate) viewport_start_line: u64,
    /// primary head 的绝对字节位（与 `selection.primary().head()` 等价）。
    pub(crate) cursor_byte: usize,
    /// primary head 的 (行号, 列号)，均 0-based；列按 Unicode scalar value 计。
    /// 底栏行:列展示直接读它，无需扫描文本。
    pub(crate) cursor_position: (u64, u64),
    pub(crate) selection: SelectionSet,
    /// 外部 reveal 请求在快照里的表示。只在多行主编辑器有意义；
    /// 嵌入式单行编辑器（搜索框等）始终为 `None`。
    pub(crate) reveal: Option<RevealHint>,
    /// BufferSearch 当前结果集中所有命中（按 ordinal 升序）。空 Vec 表示无
    /// 搜索 / 单行嵌入式输入。
    ///
    /// 由阶段 2 范围背景的第二个 producer 消费——颜色在 EditorElement::prepaint
    /// 处按 [`Self::search_current`] 区分 normal / current hit 后烤入 `Hsla`。
    pub(crate) search_hits: Vec<TextRange>,
    /// current hit 的 range；与 `search_hits` 中某一项相等（若有）。`None` 表示
    /// 无 current hit（结果集空 / 用户尚未导航）。
    pub(crate) search_current: Option<TextRange>,
}

impl EditorSnapshot {
    /// 把视口可见行用 `\n` 拼回一段字符串。
    ///
    /// 仅供老接口（blink、单行嵌入式 IME 路径、测试断言）使用——它只反映
    /// 视口内文本，对长文档不等于整 buffer 内容；新逻辑请直接消费 `lines`。
    pub(crate) fn text(&self) -> String {
        let mut buf = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                buf.push('\n');
            }
            buf.push_str(&line.text);
        }
        buf
    }
}

/// [`zom_view::RevealRequest`] 的渲染端镜像 —— 把 `ByteOffset` 换成 `usize`，
/// 便于元素侧直接用，不再跨 crate 引 engine 类型。
///
/// `line` 是 reveal 目标在整 buffer 中的逻辑行号（0-based），由 snapshot 构造
/// 时通过 `Buffer::byte_to_position` 折算出来——当 reveal 目标落在视口外时，
/// element 仍能据此把目标滚进来，而不必依赖视口切片里能不能找到该 byte。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RevealHint {
    pub(crate) byte: usize,
    pub(crate) line: u64,
    pub(crate) kind: RevealKind,
    pub(crate) seq: u64,
}

/// 视口切片器：把 `Buffer` + `SelectionSet` 投影成 `EditorSnapshot.lines` /
/// `cursor_position`。
///
/// `viewport_start_line` / `viewport_line_count` 由调用方按 View 的
/// `ViewportState` 给出；为 0 时主编辑器走默认值（见 `DEFAULT_VISIBLE_LINES`），
/// 单行嵌入式编辑器走 1。
pub(crate) fn build_snapshot_view(
    buffer: &Buffer,
    selection: &SelectionSet,
    viewport_start_line: u64,
    viewport_line_count: u64,
) -> (Vec<SnapshotLine>, u64, u64, (u64, u64), usize) {
    let total_lines = buffer.line_count() as u64;
    let cursor_byte = selection.primary().head().get();
    let cursor_position = buffer
        .byte_to_position(selection.primary().head())
        .map(|pos| (pos.line().get() as u64, pos.column().get() as u64))
        .unwrap_or((0, 0));

    if total_lines == 0 || viewport_line_count == 0 {
        return (Vec::new(), total_lines, 0, cursor_position, cursor_byte);
    }

    let clamped_start = viewport_start_line.min(total_lines.saturating_sub(1));
    let viewport = Viewport::new(
        Line::new(clamped_start as usize),
        viewport_line_count as usize,
    );
    let lines = match buffer.slice_viewport(viewport) {
        Ok(slice) => slice
            .lines()
            .iter()
            .map(|visible| SnapshotLine {
                line_index: visible.line().get() as u64,
                start_byte: visible.full_range().start().get(),
                text: visible.as_str().to_string(),
            })
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    (
        lines,
        total_lines,
        clamped_start,
        cursor_position,
        cursor_byte,
    )
}

impl Editor {
    /// 新建空编辑器。空 `Buffer` 构造不涉及 IO，不会失败。
    pub(crate) fn new() -> Self {
        let buffer = Buffer::new(BufferConfig::default()).expect("空 Buffer 构造不会失败");
        Self {
            buffer,
            selection: SelectionSet::default(),
        }
    }

    /// 当前文本内容（owned 拷贝）。
    ///
    /// 单行嵌入式编辑器（搜索框 / 文件名输入 / 项目选择器）才用它——它们的
    /// buffer 永远小，整段读取是 O(buffer size)，没有视口语义上的损失。
    /// 主编辑区不走这里，详见 [`super::main_editor`]。
    pub(crate) fn text(&self) -> String {
        self.buffer.text().into_owned()
    }

    pub(crate) fn snapshot(&self) -> EditorSnapshot {
        // 单行嵌入式编辑器没有视口语义：buffer 永远 ≤1 逻辑行，切口给个 1
        // 就能覆盖全部内容。多行主编辑器有自己的 snapshot 路径，不进这里。
        let (lines, total_lines, viewport_start_line, cursor_position, cursor_byte) =
            build_snapshot_view(
                &self.buffer,
                &self.selection,
                0,
                self.buffer.line_count() as u64,
            );
        EditorSnapshot {
            lines,
            total_lines,
            viewport_start_line,
            cursor_byte,
            cursor_position,
            selection: self.selection.clone(),
            reveal: None,
            // 嵌入式单行 Editor 不参与 BufferSearch；search overlay 永远为空。
            search_hits: Vec::new(),
            search_current: None,
        }
    }

    /// 把自身暴露成一次编辑命令的作用目标。
    pub(crate) fn as_edit_target(&mut self) -> EditTarget<'_> {
        EditTarget {
            buffer: &mut self.buffer,
            selection: &mut self.selection,
        }
    }

    /// 把自身暴露成 IME 作用目标。
    pub(crate) fn as_ime_target(&mut self) -> ImeTarget<'_> {
        ImeTarget::new(&mut self.buffer, &mut self.selection)
    }

    /// 把自身暴露成 IME 查询目标。
    pub(crate) fn as_ime_query_target(&self) -> ImeQueryTarget<'_> {
        ImeQueryTarget::new(&self.buffer, &self.selection)
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}
