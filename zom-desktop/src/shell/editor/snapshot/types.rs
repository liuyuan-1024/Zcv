//! 编辑器渲染快照类型。

use zom_engine::{SelectionSet, TextRange};
use zom_view::RevealKind;

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
    /// 完整选区集合。与 `cursor_byte` 必须从同一份 SelectionSet 派生。
    pub(crate) selection: SelectionSet,
    /// 外部 reveal 请求在快照里的表示。是否生效由 `EditorKernel` 决定。
    pub(crate) reveal: Option<RevealHint>,
    /// BufferSearch 当前结果集中所有命中（按 ordinal 升序）。
    pub(crate) search_hits: Vec<TextRange>,
    /// current hit 的 range；与 `search_hits` 中某一项相等（若有）。
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RevealHint {
    pub(crate) byte: usize,
    pub(crate) line: u64,
    pub(crate) kind: RevealKind,
    pub(crate) seq: u64,
}
