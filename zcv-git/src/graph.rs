//! git graph 的 lane 布局算法：把线性排列的提交序列转换为逐行的图形绘制指令。
//!
//! 纯计算、不依赖 gpui，可脱离 UI 单测。
//! 算法参考经典 `git log --graph`：
//! 为每条"已画出但尚未落地的父提交边"分配一条 lane，提交出现时确定自身所在 lane、汇入所有等待它的边、再为自己的父提交延续（第一父）或分叉（其余父）出新的 lane。

use crate::repository::GraphCommit;

/// 一行图形中需要绘制的线段。
///
/// 以行中央的圆点为界：`MergeIn` 位于上半行、`ForkOut` 位于下半行、`Pass` 贯穿整行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphLine {
    /// 竖直贯穿整行（行顶到行底）：一条与本提交无关、仍在等待其父出现的边。
    Pass { lane: usize, color: usize },
    /// 上半行汇入：从行顶的 `from_lane` 连到中央圆点（`from_lane == dot_lane` 时为竖直上半）。
    MergeIn { from_lane: usize, color: usize },
    /// 下半行分叉：从中央圆点连到行底的 `to_lane`（`to_lane == dot_lane` 时为竖直下半）。
    ForkOut { to_lane: usize, color: usize },
}

/// 单个提交的逐行布局结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRowLayout {
    /// 圆点所在 lane。
    pub dot_lane: usize,
    /// 圆点及其第一父边的配色索引（视图侧对配色板长度取模）。
    pub dot_color: usize,
    /// 本行需绘制的线段。
    pub lines: Vec<GraphLine>,
    /// 截至本行的 lane 总数（含尾部空闲 lane），供视图计算画布宽度。
    pub max_lanes: usize,
}

/// 一条 lane 的状态：正在等待出现的父提交 oid 及其配色。
#[derive(Clone, Debug)]
struct LaneState {
    waiting: Option<String>,
    color: usize,
}

/// 跨提交（含跨批次）承载 lane 占用状态的布局器。
///
/// 按 `git log` 输出顺序逐个 [`push`](Self::push) 提交，得到每行的绘制指令；
/// 分批加载时复用同一实例，第二批首提交即可匹配第一批末尾留下的等待边。
#[derive(Clone, Debug, Default)]
pub struct GraphLayoutState {
    lanes: Vec<LaneState>,
    next_color: usize,
}

impl GraphLayoutState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 首个空闲 lane（`waiting` 为 None）；无空闲则在末尾新建一条。
    fn first_empty_lane(&mut self) -> usize {
        if let Some(index) = self.lanes.iter().position(|lane| lane.waiting.is_none()) {
            index
        } else {
            self.lanes.push(LaneState {
                waiting: None,
                color: 0,
            });
            self.lanes.len() - 1
        }
    }

    /// 分配下一个配色索引（单调递增，取模配色板由视图侧负责）。
    fn alloc_color(&mut self) -> usize {
        let color = self.next_color;
        self.next_color += 1;
        color
    }

    /// 处理一个提交，返回其所在行的绘制指令，并推进 lane 状态到下一行。
    pub fn push(&mut self, commit: &GraphCommit) -> GraphRowLayout {
        // 等待本提交出现的 lane：即把本提交列为父、已画出但尚未落地的边。
        let incoming: Vec<usize> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| lane.waiting.as_deref() == Some(commit.oid.as_str()))
            .map(|(index, _)| index)
            .collect();

        // 提交所在 lane：优先复用汇入边中最小的一条，否则取首个空闲 lane（新分支起点）。
        let from_incoming = !incoming.is_empty();
        let commit_lane = incoming
            .first()
            .copied()
            .unwrap_or_else(|| self.first_empty_lane());

        // 延续汇入边的配色；新分支（顶部提交或 merge 引入的分支）分配新色。
        let dot_color = if from_incoming {
            self.lanes[commit_lane].color
        } else {
            let color = self.alloc_color();
            self.lanes[commit_lane].color = color;
            color
        };

        let mut lines = Vec::new();

        // 上半行：每条汇入边从行顶连到圆点（commit_lane 自身即竖直上半）。
        for lane in &incoming {
            let color = self.lanes[*lane].color;
            lines.push(GraphLine::MergeIn {
                from_lane: *lane,
                color,
            });
        }

        // 竖直贯穿：与本提交无关、仍在等待各自父出现的 active lane。
        for lane in 0..self.lanes.len() {
            if lane == commit_lane || incoming.contains(&lane) {
                continue;
            }
            if self.lanes[lane].waiting.is_some() {
                let color = self.lanes[lane].color;
                lines.push(GraphLine::Pass { lane, color });
            }
        }

        // 释放全部汇入 lane（含 commit_lane），随后按父提交重新设置。
        for lane in &incoming {
            self.lanes[*lane].waiting = None;
        }

        // 下半行：第一父继承 commit_lane，其余父（merge）各分叉出一条新 lane。
        match commit.parents.split_first() {
            Some((first, rest)) => {
                self.lanes[commit_lane] = LaneState {
                    waiting: Some(first.clone()),
                    color: dot_color,
                };
                lines.push(GraphLine::ForkOut {
                    to_lane: commit_lane,
                    color: dot_color,
                });
                for parent in rest {
                    let lane = self.first_empty_lane();
                    let color = self.alloc_color();
                    self.lanes[lane] = LaneState {
                        waiting: Some(parent.clone()),
                        color,
                    };
                    lines.push(GraphLine::ForkOut {
                        to_lane: lane,
                        color,
                    });
                }
            }
            // 根提交：无父，圆点下方无线，commit_lane 释放为空闲。
            None => self.lanes[commit_lane].waiting = None,
        }

        GraphRowLayout {
            dot_lane: commit_lane,
            dot_color,
            lines,
            max_lanes: self.lanes.len(),
        }
    }
}

#[cfg(test)]
mod tests {
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
}
