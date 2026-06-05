//! 回归测试：编辑当帧不要露出默认前景色。
//!
//! confirmed layer 保存 worker 已确认的权威 span；provisional layer 保存 UI 线程
//! 在 worker 回来前对新写入字节的临时样式推断。边界插入时 confirmed 会按
//! `Stickiness::Never` 保持严格范围，provisional 必须立即补上新字节。

use std::path::PathBuf;

use zom_workspace::Workspace;
use zom_workspace::syntax::{
    LanguageDetector, LanguageId, providers::markdown, syntax_confirmed_layer_kind,
    syntax_provisional_layer_kind,
};

const INITIAL_TEXT: &str = "# zom 文档规范\n\n正文。\n";

fn make_workspace_with_markdown() -> Workspace {
    let mut workspace = Workspace::new();
    workspace.language_registry_mut().register(
        LanguageId::new("markdown"),
        vec![LanguageDetector::Extension(&["md"])],
        Box::new(|| Box::new(markdown::new_provider())),
    );
    workspace
}

fn open_markdown_with_initial_highlights() -> (Workspace, zom_workspace::BufferId) {
    let mut workspace = make_workspace_with_markdown();
    let id = workspace
        .open_text(Some(PathBuf::from("repro.md")), INITIAL_TEXT)
        .unwrap();
    workspace.syntax_worker().wait_for_idle();
    workspace.pump_pending_highlights();

    let wb = workspace.buffer(id).unwrap();
    let initial_spans: Vec<(usize, usize)> = confirmed_ranges(wb);
    assert!(
        initial_spans.iter().any(|(s, e)| *s == 2 && *e == 18),
        "tree-sitter-md 应给出 「zom 文档规范」=[2, 18) heading 标记，实际 spans={initial_spans:?}",
    );
    (workspace, id)
}

fn insert_text(workspace: &mut Workspace, id: zom_workspace::BufferId, byte: usize, text: &str) {
    let wb_mut = workspace.buffer_mut(id).unwrap();
    let buf = wb_mut.buffer_mut();
    use zom_engine::{ByteOffset, Edit, Transaction};
    let edit = Edit::insert(ByteOffset::new(byte), text.to_string()).unwrap();
    let tx = Transaction::from_edits(buf.version(), vec![edit]).unwrap();
    buf.apply_transaction(tx).unwrap();
    workspace.buffer_mut(id).unwrap().pump_post_edit().unwrap();
}

fn confirmed_ranges(wb: &zom_workspace::WorkspaceBuffer) -> Vec<(usize, usize)> {
    wb.highlight_layers()
        .layer(&syntax_confirmed_layer_kind())
        .expect("confirmed syntax layer 必须存在")
        .as_slice()
        .iter()
        .map(|r| (r.range().start().get(), r.range().end().get()))
        .collect()
}

fn provisional_ranges(wb: &zom_workspace::WorkspaceBuffer) -> Vec<(usize, usize)> {
    wb.highlight_layers()
        .layer(&syntax_provisional_layer_kind())
        .map(|layer| {
            layer
                .as_slice()
                .iter()
                .map(|r| (r.range().start().get(), r.range().end().get()))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn pump_post_edit_keeps_insert_inside_token_highlighted_without_worker() {
    let (mut workspace, id) = open_markdown_with_initial_highlights();
    let wb = workspace.buffer(id).unwrap();
    let initial_version = wb.buffer().version();

    // 插在 heading span 内部时，confirmed remap 就应立即覆盖新字节。
    insert_text(&mut workspace, id, 5, "X");

    let wb = workspace.buffer(id).unwrap();
    assert_ne!(
        wb.buffer().version(),
        initial_version,
        "buffer 版本必须推进"
    );
    let confirmed = confirmed_ranges(wb);
    assert!(
        confirmed.iter().any(|(s, e)| *s == 2 && *e == 19),
        "confirmed layer 应立即覆盖插入后的 heading [2, 19)，实际 spans={confirmed:?}",
    );
    assert!(
        provisional_ranges(wb).is_empty(),
        "confirmed 已覆盖新字节时不需要额外 provisional span"
    );
}

#[test]
fn pump_post_edit_adds_provisional_span_for_boundary_insert() {
    let (mut workspace, id) = open_markdown_with_initial_highlights();
    let wb = workspace.buffer(id).unwrap();
    let initial_version = wb.buffer().version();
    // 在 heading span 右边界插入一个 ASCII 字符，模拟继续输入标题。
    // 之后立刻 pump_post_edit，但不等 worker 重算（worker 是异步的，render
    // 与 worker 之间天然存在 1–N 帧延迟，provisional 必须覆盖这一帧）。
    insert_text(&mut workspace, id, 18, "X");

    let wb = workspace.buffer(id).unwrap();
    let buf = wb.buffer();
    assert_ne!(buf.version(), initial_version, "buffer 版本必须推进");

    let layer = wb
        .highlight_layers()
        .layer(&syntax_confirmed_layer_kind())
        .unwrap();
    assert_eq!(
        layer.version(),
        buf.version(),
        "confirmed syntax layer 应随编辑推进到当前版本",
    );
    assert!(
        layer
            .as_slice()
            .iter()
            .any(|r| { r.range().start().get() == 2 && r.range().end().get() == 18 }),
        "confirmed layer 应保持旧 heading 的严格边界，实际 spans={:?}",
        layer
            .as_slice()
            .iter()
            .map(|r| (r.range().start().get(), r.range().end().get()))
            .collect::<Vec<_>>(),
    );
    let provisional = provisional_ranges(wb);
    assert!(
        provisional.iter().any(|(s, e)| *s == 18 && *e == 19),
        "provisional layer 应立即覆盖新插入字节 [18, 19)，实际 spans={:?}",
        provisional,
    );

    workspace.syntax_worker().wait_for_idle();
    workspace.pump_pending_highlights();

    let wb = workspace.buffer(id).unwrap();
    let buf = wb.buffer();
    let layer = wb
        .highlight_layers()
        .layer(&syntax_confirmed_layer_kind())
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
    assert!(
        wb.highlight_layers()
            .layer(&syntax_provisional_layer_kind())
            .map(|layer| layer.is_empty())
            .unwrap_or(true),
        "worker 的正式结果落地后 provisional layer 应清空",
    );
}

#[test]
fn consecutive_boundary_inserts_extend_provisional_without_worker() {
    let (mut workspace, id) = open_markdown_with_initial_highlights();

    insert_text(&mut workspace, id, 18, "X");
    insert_text(&mut workspace, id, 19, "Y");

    let wb = workspace.buffer(id).unwrap();
    let provisional = provisional_ranges(wb);
    assert!(
        provisional.iter().any(|(s, e)| *s == 18 && *e == 19),
        "第一次边界输入应保留 provisional [18, 19)，实际 spans={provisional:?}",
    );
    assert!(
        provisional.iter().any(|(s, e)| *s == 19 && *e == 20),
        "第二次连续输入应从旧 provisional 继承样式并覆盖 [19, 20)，实际 spans={provisional:?}",
    );
}
