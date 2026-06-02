//! 回归测试：编辑后、syntax worker 新产物落地前，pump_post_edit 必须把
//! highlight_layers 现有 span 沿 DeltaEvent 平移到新版本，否则旧字节偏移会
//! 在多字节字符中间被切开，桌面端 `build_text_runs_for_line` 给 gpui 的
//! TextRun 端点落进 char 内部，shape_line panic（详见高亮架构手册 §五）。

use std::path::PathBuf;

use zom_workspace::Workspace;
use zom_workspace::syntax::{LanguageDetector, LanguageId, providers::markdown, syntax_layer_kind};

fn make_workspace_with_markdown() -> Workspace {
    let mut workspace = Workspace::new();
    workspace.language_registry_mut().register(
        LanguageId::new("markdown"),
        vec![LanguageDetector::Extension(&["md"])],
        Box::new(|| Box::new(markdown::new_provider())),
    );
    workspace
}

#[test]
fn pump_post_edit_shifts_syntax_spans_through_multibyte_insert() {
    let mut workspace = make_workspace_with_markdown();
    // 用 .md 扩展名走 markdown provider。文件首行含 4 个中文字符的标题。
    let id = workspace
        .open_text(
            Some(PathBuf::from("repro.md")),
            "# zom 文档规范\n\n正文。\n",
        )
        .unwrap();

    // 等 worker 算出初始 spans，drain 进 layers。
    workspace.syntax_worker().wait_for_idle();
    workspace.pump_pending_highlights();

    let wb = workspace.buffer(id).unwrap();
    let initial_version = wb.buffer().version();
    let initial_spans: Vec<(usize, usize)> = wb
        .highlight_layers()
        .layer(&syntax_layer_kind())
        .expect("syntax layer 必须挂上")
        .as_slice()
        .iter()
        .map(|r| (r.range().start().get(), r.range().end().get()))
        .collect();
    assert!(
        initial_spans.iter().any(|(s, e)| *s == 2 && *e == 18),
        "tree-sitter-md 应给出 「zom 文档规范」=[2, 18) heading 标记，实际 spans={initial_spans:?}",
    );

    // 在 byte 5（`m` 后）插入一个 ASCII 字符，模拟一次按键。
    // 之后立刻 pump_post_edit，但不等 worker 重算（worker 是异步的，render
    // 与 worker 之间天然存在 1–N 帧延迟，正是 crash 现场）。
    {
        let wb_mut = workspace.buffer_mut(id).unwrap();
        let buf = wb_mut.buffer_mut();
        use zom_engine::{ByteOffset, Edit, Transaction};
        let edit = Edit::insert(ByteOffset::new(5), "X".to_string()).unwrap();
        let tx = Transaction::from_edits(buf.version(), vec![edit]).unwrap();
        buf.apply_transaction(tx).unwrap();
    }
    workspace.buffer_mut(id).unwrap().pump_post_edit().unwrap();

    let wb = workspace.buffer(id).unwrap();
    let buf = wb.buffer();
    assert_ne!(buf.version(), initial_version, "buffer 版本必须推进");

    // 拿新文本，验证 layer 现存 span 全在新文本的 char boundary 上。
    let new_text = buf
        .slice_byte_range(zom_engine::ByteOffset::ZERO, buf.len_bytes())
        .unwrap()
        .into_text()
        .into_owned();
    let layer = wb
        .highlight_layers()
        .layer(&syntax_layer_kind())
        .expect("syntax layer 仍在");
    for entry in layer.as_slice() {
        let s = entry.range().start().get();
        let e = entry.range().end().get();
        assert!(
            new_text.is_char_boundary(s),
            "span start {s} 落在新文本 char 中间：text={new_text:?}",
        );
        assert!(
            e == new_text.len() || new_text.is_char_boundary(e),
            "span end {e} 落在新文本 char 中间：text={new_text:?}",
        );
    }

    // 同时验证：旧 [2, 18) heading span 已被平移到 [2, 19)——end=19 在新文本
    // 里正好是首行 newline 字节（char-aligned）。回归这个具体偏移就能盯住
    // position_map 行为，将来位移逻辑改了能立刻警示。
    let shifted: Vec<(usize, usize)> = layer
        .as_slice()
        .iter()
        .map(|r| (r.range().start().get(), r.range().end().get()))
        .collect();
    assert!(
        shifted.iter().any(|(s, e)| *s == 2 && *e == 19),
        "[2, 18) heading 应平移到 [2, 19)，实际 spans={shifted:?}",
    );
}
