//! M13D 机器契约：锁定 ProjectedViewport / ProjectedViewportSlice 折叠后视口切片语义。
//!
//! 验证范围：
//! - 按 projected line range 切片：精确返回 viewport 内每条投影行（text 或 placeholder）;
//! - viewport 自动 clamp 到投影空间末尾，不越界报错;
//! - 文本行视为 `VisibleLine`，应用 max_line_chars 截断；超长行 `is_truncated` 为 true;
//! - placeholder 行原样穿透，并被汇总到 `placeholders()` 列表;
//! - 视口对应的逻辑行 spans 把连续可见逻辑行合并成 LineRange;
//! - 跨 fold 的视口同时枚举 text + placeholder 行;
//! - snapshot 与 projection 版本不一致时切片原子拒绝。

use zom_engine::{
    Buffer, BufferConfig, Edit, EngineError, FoldSet, Line, LineRange, ProjectedLineIndex,
    ProjectedViewport, ProjectedViewportRowKind, Projection, ProjectionError, Transaction,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn line(value: usize) -> Line {
    Line::new(value)
}

fn line_range(start: usize, end: usize) -> LineRange {
    LineRange::new(line(start), line(end)).unwrap()
}

fn projected(idx: usize) -> ProjectedLineIndex {
    ProjectedLineIndex::new(idx)
}

fn apply(buffer: &mut Buffer, edits: Vec<Edit>) {
    let tx = Transaction::from_edits(buffer.version(), edits).unwrap();
    buffer.apply_transaction(tx).unwrap();
}

fn folded_buffer() -> (Buffer, FoldSet, Projection) {
    // 11 条逻辑行（编号 0..=10），fold 隐藏 logical line 4..=7。
    let text = "L0\nL1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\nL9\nL10\n";
    let buffer = buffer(text);
    let mut folds = FoldSet::new(buffer.version());
    folds.fold_lines(&buffer, line_range(3, 8)).unwrap();
    let projection = Projection::build(&buffer.snapshot(), &folds).unwrap();
    // 投影行：proj 0=L0 ... proj 3=L3 (anchor) proj 4=Placeholder proj 5=L8 proj 6=L9 proj 7=L10 proj 8=空末行
    (buffer, folds, projection)
}

#[test]
fn viewport_returns_consecutive_text_rows_for_unfolded_region() {
    let (buffer, _folds, projection) = folded_buffer();
    let snapshot = buffer.snapshot();
    let viewport = ProjectedViewport::new(projected(0), 3);
    let slice = projection.slice_viewport(&snapshot, viewport).unwrap();

    assert_eq!(slice.len(), 3);
    for (idx, row) in slice.rows().iter().enumerate() {
        assert_eq!(row.index(), projected(idx));
        let kind = row.kind();
        assert!(kind.is_text());
        let visible = kind.visible_line().unwrap();
        assert_eq!(visible.line(), line(idx));
        assert_eq!(visible.as_str(), &format!("L{idx}"));
        assert!(!visible.is_truncated());
    }
    assert!(slice.placeholders().is_empty());
    assert_eq!(slice.logical_line_spans(), &[line_range(0, 3)]);
}

#[test]
fn viewport_emits_placeholder_row_when_window_includes_fold() {
    let (buffer, _folds, projection) = folded_buffer();
    let snapshot = buffer.snapshot();
    let viewport = ProjectedViewport::new(projected(2), 5);
    let slice = projection.slice_viewport(&snapshot, viewport).unwrap();

    // proj 2=L2, proj 3=L3 anchor, proj 4=placeholder, proj 5=L8, proj 6=L9
    assert_eq!(slice.len(), 5);
    assert!(slice.rows()[0].kind().is_text());
    assert_eq!(
        slice.rows()[0].kind().visible_line().unwrap().line(),
        line(2)
    );
    assert!(slice.rows()[1].kind().is_text());
    assert_eq!(
        slice.rows()[1].kind().visible_line().unwrap().line(),
        line(3)
    );
    assert!(slice.rows()[2].kind().is_placeholder());
    let placeholder = slice.rows()[2].kind().placeholder().unwrap();
    assert_eq!(placeholder.anchor_line(), line(3));
    assert_eq!(placeholder.hidden_lines(), line_range(4, 8));
    assert!(slice.rows()[3].kind().is_text());
    assert_eq!(
        slice.rows()[3].kind().visible_line().unwrap().line(),
        line(8)
    );
    assert!(slice.rows()[4].kind().is_text());
    assert_eq!(
        slice.rows()[4].kind().visible_line().unwrap().line(),
        line(9)
    );

    assert_eq!(slice.placeholders().len(), 1);
    assert_eq!(slice.placeholders()[0], placeholder);
    // 视口逻辑行 spans：L2, L3 连续 + L8, L9 连续
    assert_eq!(
        slice.logical_line_spans(),
        &[line_range(2, 4), line_range(8, 10)]
    );
}

#[test]
fn viewport_clamps_line_count_at_projection_end_without_error() {
    let (buffer, _folds, projection) = folded_buffer();
    let snapshot = buffer.snapshot();
    let total = projection.line_count();
    // 请求超出投影空间的视口长度
    let viewport = ProjectedViewport::new(projected(total - 2), 100);
    let slice = projection.slice_viewport(&snapshot, viewport).unwrap();

    assert_eq!(slice.len(), 2);
    assert_eq!(slice.projected_line_range().end(), projected(total));
}

#[test]
fn viewport_starting_past_projection_end_returns_coordinate_error() {
    let (buffer, _folds, projection) = folded_buffer();
    let total = projection.line_count();
    let snapshot = buffer.snapshot();
    let viewport = ProjectedViewport::new(projected(total + 5), 1);
    let err = projection.slice_viewport(&snapshot, viewport).unwrap_err();
    assert!(matches!(err, EngineError::Coordinate(_)));
}

#[test]
fn viewport_max_line_chars_truncates_long_visible_lines() {
    let buffer = buffer("aaaaaaaaaa\nbbbb\n");
    let folds = FoldSet::new(buffer.version());
    let snapshot = buffer.snapshot();
    let projection = Projection::build(&snapshot, &folds).unwrap();
    let viewport = ProjectedViewport::new(projected(0), 2).with_max_line_chars(4);
    let slice = projection.slice_viewport(&snapshot, viewport).unwrap();

    let first = slice.rows()[0].kind().visible_line().unwrap();
    assert_eq!(first.as_str(), "aaaa");
    assert!(first.is_truncated());

    let second = slice.rows()[1].kind().visible_line().unwrap();
    assert_eq!(second.as_str(), "bbbb");
    assert!(!second.is_truncated());
}

#[test]
fn viewport_logical_spans_skip_placeholder_rows_and_split_at_folds() {
    let (buffer, _folds, projection) = folded_buffer();
    let snapshot = buffer.snapshot();
    // 整个投影空间
    let viewport = ProjectedViewport::new(projected(0), projection.line_count());
    let slice = projection.slice_viewport(&snapshot, viewport).unwrap();

    assert_eq!(
        slice.logical_line_spans(),
        &[line_range(0, 4), line_range(8, 12)]
    );
    assert_eq!(slice.placeholders().len(), 1);
    let placeholder = slice.placeholders()[0];
    assert_eq!(placeholder.anchor_line(), line(3));
    assert_eq!(placeholder.hidden_lines(), line_range(4, 8));
}

#[test]
fn viewport_rejects_snapshot_with_different_version() {
    let (mut buffer, _folds, projection) = folded_buffer();
    apply(
        &mut buffer,
        vec![Edit::insert(zom_engine::CharOffset::new(0), "X".to_string()).unwrap()],
    );
    let stale_snapshot = buffer.snapshot();
    let viewport = ProjectedViewport::new(projected(0), 3);
    let err = projection
        .slice_viewport(&stale_snapshot, viewport)
        .unwrap_err();
    assert!(matches!(
        err,
        EngineError::Projection(ProjectionError::VersionMismatch { .. })
    ));
}

#[test]
fn viewport_rows_can_be_destructured_into_text_or_placeholder_kind() {
    let (buffer, _folds, projection) = folded_buffer();
    let snapshot = buffer.snapshot();
    let viewport = ProjectedViewport::new(projected(3), 2);
    let slice = projection.slice_viewport(&snapshot, viewport).unwrap();
    assert_eq!(slice.len(), 2);
    let mut iter = slice.into_rows().into_iter();
    let anchor_row = iter.next().unwrap();
    match anchor_row.into_kind() {
        ProjectedViewportRowKind::Text {
            logical_line,
            visible,
        } => {
            assert_eq!(logical_line, line(3));
            assert_eq!(visible.as_str(), "L3");
        }
        _ => panic!("expected text row"),
    }
    let placeholder_row = iter.next().unwrap();
    match placeholder_row.into_kind() {
        ProjectedViewportRowKind::Placeholder(placeholder) => {
            assert_eq!(placeholder.anchor_line(), line(3));
            assert_eq!(placeholder.hidden_lines(), line_range(4, 8));
        }
        _ => panic!("expected placeholder row"),
    }
}
