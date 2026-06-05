//! 回归测试：编辑后不要在 UI 线程同步 remap 整个 syntax layer。
//!
//! 旧契约曾要求 `pump_post_edit` 立刻把所有高亮 span 沿 DeltaEvent 平移，以避免
//! 旧字节偏移切进 UTF-8 字符中间。桌面端消费高亮时已经会夹到当前行并对齐
//! char boundary，因此这里应选择更便宜的策略：编辑当帧清空旧 syntax layer，
//! 再等 worker 用新版本产物补回。

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
fn pump_post_edit_clears_syntax_spans_until_worker_replaces_them() {
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

    let layer = wb
        .highlight_layers()
        .layer(&syntax_layer_kind())
        .expect("syntax layer 仍在");
    assert_eq!(
        layer.version(),
        buf.version(),
        "清空后的 syntax layer 应推进到当前版本，方便 worker ReplaceRange 落地",
    );
    assert!(
        layer.as_slice().is_empty(),
        "pump_post_edit 不应同步 remap 旧高亮 span，实际 spans={:?}",
        layer
            .as_slice()
            .iter()
            .map(|r| (r.range().start().get(), r.range().end().get()))
            .collect::<Vec<_>>(),
    );

    workspace.syntax_worker().wait_for_idle();
    workspace.pump_pending_highlights();

    let wb = workspace.buffer(id).unwrap();
    let buf = wb.buffer();
    let layer = wb
        .highlight_layers()
        .layer(&syntax_layer_kind())
        .expect("worker 应补回 syntax layer");

    // 拿新文本，验证 worker 补回的 span 全在新文本的 char boundary 上。
    let new_text = buf
        .slice_byte_range(zom_engine::ByteOffset::ZERO, buf.len_bytes())
        .unwrap()
        .into_text()
        .into_owned();
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

    // worker 基于新文本重算后，heading span 应覆盖插入后的标题文本。
    let refreshed: Vec<(usize, usize)> = layer
        .as_slice()
        .iter()
        .map(|r| (r.range().start().get(), r.range().end().get()))
        .collect();
    assert!(
        refreshed.iter().any(|(s, e)| *s == 2 && *e == 19),
        "新版本 heading 应覆盖 [2, 19)，实际 spans={refreshed:?}",
    );
}
