use super::*;

/// 构造只含 oid 与父提交的 GraphCommit（其余字段与布局无关）。
fn commit(oid: &str, parents: &[&str]) -> GraphCommit {
    GraphCommit {
        oid: oid.to_string(),
        parents: parents.iter().map(|parent| parent.to_string()).collect(),
        author_name: String::new(),
        timestamp: 0,
        subject: String::new(),
        refs: Vec::new(),
    }
}

#[test]
fn linear_history_stays_on_single_lane() {
    // git log 顺序：C(HEAD) → B → A(根)。
    let mut state = GraphLayoutState::new();

    // 顶部提交 C：无汇入（上半无线），下半连向父 B。
    let row_c = state.push(&commit("C", &["B"]));
    assert_eq!(row_c.dot_lane, 0);
    assert_eq!(
        row_c.lines,
        vec![GraphLine::ForkOut {
            to_lane: 0,
            color: 0
        }]
    );

    // 中间提交 B：上半来自 C 的汇入（竖直），下半连向父 A。
    let row_b = state.push(&commit("B", &["A"]));
    assert_eq!(row_b.dot_lane, 0);
    assert_eq!(
        row_b.lines,
        vec![
            GraphLine::MergeIn {
                from_lane: 0,
                color: 0
            },
            GraphLine::ForkOut {
                to_lane: 0,
                color: 0
            }
        ]
    );

    // 根提交 A：上半来自 B 的汇入，下半无线。
    let row_a = state.push(&commit("A", &[]));
    assert_eq!(row_a.dot_lane, 0);
    assert_eq!(
        row_a.lines,
        vec![GraphLine::MergeIn {
            from_lane: 0,
            color: 0
        }]
    );
    assert_eq!(row_a.max_lanes, 1, "线性历史始终只有一条 lane");
}

#[test]
fn merge_commit_forks_two_lanes_and_reunites() {
    // 历史：M(merge, 父=[X, Y])；X、Y 同源于 B；B → root。
    // git log 顺序：M, Y, X, B, root。
    let mut state = GraphLayoutState::new();

    // M：顶部提交，第一父 X 继承 lane0，第二父 Y 分叉出 lane1。
    let row_m = state.push(&commit("M", &["X", "Y"]));
    assert_eq!(row_m.dot_lane, 0);
    assert_eq!(
        row_m.lines,
        vec![
            GraphLine::ForkOut {
                to_lane: 0,
                color: 0
            },
            GraphLine::ForkOut {
                to_lane: 1,
                color: 1
            }
        ]
    );
    assert_eq!(row_m.max_lanes, 2);

    // Y：落在 lane1（M 分叉出的边），lane0（等待 X）竖直贯穿。
    let row_y = state.push(&commit("Y", &["B"]));
    assert_eq!(row_y.dot_lane, 1);
    assert!(row_y.lines.contains(&GraphLine::Pass { lane: 0, color: 0 }));
    assert!(row_y.lines.contains(&GraphLine::MergeIn {
        from_lane: 1,
        color: 1
    }));
    assert!(row_y.lines.contains(&GraphLine::ForkOut {
        to_lane: 1,
        color: 1
    }));

    // X：落在 lane0，lane1（现等待 B）竖直贯穿。
    let row_x = state.push(&commit("X", &["B"]));
    assert_eq!(row_x.dot_lane, 0);
    assert!(row_x.lines.contains(&GraphLine::Pass { lane: 1, color: 1 }));
    assert!(row_x.lines.contains(&GraphLine::MergeIn {
        from_lane: 0,
        color: 0
    }));

    // B：被 lane0(X) 与 lane1(Y) 同时等待，两条边一并汇入。
    let row_b = state.push(&commit("B", &["root"]));
    assert_eq!(row_b.dot_lane, 0);
    let merges: Vec<_> = row_b
        .lines
        .iter()
        .filter(|line| matches!(line, GraphLine::MergeIn { .. }))
        .collect();
    assert_eq!(merges.len(), 2, "merge 汇合点应有两条汇入边");
    assert!(merges.contains(&&GraphLine::MergeIn {
        from_lane: 0,
        color: 0
    }));
    assert!(merges.contains(&&GraphLine::MergeIn {
        from_lane: 1,
        color: 1
    }));
}

#[test]
fn state_continues_across_batches() {
    // 分批加载：第一批只到 C，第二批从 B 开始，复用同一 state。
    let mut state = GraphLayoutState::new();
    let _row_c = state.push(&commit("C", &["B"]));
    // 第一批结束时 lane0 仍在等待 B。

    // 第二批首个提交 B 应匹配上一批留下的等待边（上半有汇入）。
    let row_b = state.push(&commit("B", &["A"]));
    assert_eq!(row_b.dot_lane, 0);
    assert!(
        row_b.lines.contains(&GraphLine::MergeIn {
            from_lane: 0,
            color: 0
        }),
        "跨批次时 B 应衔接上一批留下的等待边"
    );
}
