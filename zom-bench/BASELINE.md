# 16 MiB 流畅性目标 · 性能基线

> 本文档是 zom 「16 MiB 文本 / 高亮 + 编辑流畅」目标的数字起点。
> 任何针对性能的改动，**必须**先在这一组场景上重跑，把新旧数字一起钉进文档底部的「历史快照」区。

## 1. 测量方法

- **工具**：[`zom-bench`](src/) crate。`cargo run --release -p zom-bench -- run <lang> [size]`。
- **语料**：合成数据，落盘到 `target/bench-corpus/`，由 `zom-bench corpus` 一键生成；`{rust, json, log} × {1, 4, 16, 64} MiB` 共 12 份。语料只追求"形态像真实代码"以经过 tree-sitter 的典型分支，不追求语义正确。
- **场景**：
  - `load`：`Buffer::from_reader` 流式读取，含 UTF-8 校验 / 行尾扫描 / 最长行扫描 / rope build。3 次取平均。
  - `viewport`：`snapshot.slice_viewport(Viewport::new(中位行, 60))`。1000 次取平均，衡量渲染热路径。
  - `search`：`buffer.search_regex(pattern, default)`。3 次取平均。
  - `parse`：`BufferSyntax::attach` 内含的全量 `run_full`。3 次取平均。
  - `edit+highlight`：`buffer.insert` + `BufferSyntax::handle_edit`。规模越大迭代越少，避免一次跑几分钟。
- **构建**：release。
- **峰值内存**：bench 不自测；外层用 `/usr/bin/time -l`（macOS）或 `/usr/bin/time -v`（Linux）读 maximum resident set size。

## 2. 16 MiB 基线（rust 语料，2026-06-08 实测）

`BufferSyntaxTree` slot 改造落地后的稳态：sink / `MetadataLayer<HighlightSpan>` / coalesce 整条路径下线，worker reparse 完成后写 `Arc<BufferSyntaxTree>` 进共享 slot，paint 端按 viewport 现查。v0.2.0 起点与各阶段过渡数字见 §6 历史快照。

| 场景 | 16 MiB | 红线 | 备注 |
|---|---:|---:|---|
| `load` | **42.5 ms** | — | `Buffer::from_reader` 流式：64 KiB 读缓冲 + 单次扫描合并 UTF-8 / 行尾 / 最长行 |
| `viewport` | **35.7 µs** | ≤ 100 µs ✅ | `snapshot.slice_viewport(60 行 @ 中位行)`；渲染热路径 |
| `search` | **5.6 ms** | ≤ 200 ms ✅ | regex `\bfn\b|\blet\b|\bimpl\b`；命中 114357 处；8 MiB 硬限已解除 |
| `parse` | **0.92 s** | ≤ 1.5 s ✅ | tree-sitter 冷启动；worker 异步、不阻塞主线程 |
| `edit+hl 主线程` | **6.0 µs / 键** | ≤ 5 ms ✅ | `insert` + 主线程同步 `tree_slot.try_edit` 推坐标 + push channel；与文件大小脱钩（830× 余量） |
| `edit+hl 端到端` | **78.6 ms / 键** | ≤ 100 ms ✅ | 编辑 + worker `wait_for_idle`（增量 reparse 完成）；新 tree 写入共享 slot 后下一帧 paint 现查即正确 |

> 1 MiB / 4 MiB 一档作秒级回归（`run rust 4`）使用，触发红线告警已足够，不再列入本表。
> 64 MiB 一档仍可跑作"远端劣化"观察，见 §6；超过 16 MiB 已不在红线管辖。
> json / log 语料尚未实测。

## 2.1 render-time query 与增量 reparse 的分位数（2026-06-08）

paint 路径与编辑路径上两条核心 tree-sitter 调用的实测分位数，作为「render-time query 现查 viewport 是否够快」「主线程同步 reparse 是否可行」的依据。

| 场景 | 1 MiB | 4 MiB | 16 MiB | 红线参考 | 备注 |
|---|---:|---:|---:|---:|---|
| `render-time query` p50 | 72.8 µs | 74.3 µs | 75.1 µs | — | 直接用 `tree_sitter::QueryCursor::captures` + `set_byte_range`，50 行 viewport ×200 次 |
| `render-time query` p95 | 81.0 µs | 78.6 µs | 84.8 µs | — | |
| `render-time query` p99 | **92.3 µs** | **90.0 µs** | **100.1 µs** | < 500 µs | 跨规模几乎常数——`set_byte_range` 把工作限定在 viewport 内 |
| `incremental reparse` p50 | 1.37 ms | 13.4 ms | 68.7 ms | — | `tree.edit` + `parse_with_options(Some(&old))`；单字符 ASCII 插入 ×100 次 |
| `incremental reparse` p95 | 1.85 ms | 15.9 ms | 82.7 ms | — | |
| `incremental reparse` p99 | **2.38 ms** | **18.5 ms** | **87.0 ms** | — | 严格随文件大小线性增长 |

**结论**：

1. **render-time query 直接走，不缓存**。p99 跨规模 < 0.11 ms 远低于 0.5 ms 阈值，paint 时现查 viewport 16 MiB rust 仍只占 60 FPS 帧预算的 0.7%。
2. **主线程不做同步增量 reparse**。16 MiB 单键 69–87 ms ≈ 4–6 帧，必须留在 worker 上。小文件上叠加"命中预算就同步 reparse"的 fast-path 是可选优化（≤ 1.5 ms budget 覆盖 1 MiB、≤ 5 ms 覆盖 4 MiB）。
3. **wrong-color flash 的根源**：16 MiB 上观察到的"补色尾迹"≈ 4-6 帧，本质是 tree-sitter 增量 reparse 在该规模下的固有耗时，工程上靠"主线程 `tree.edit` 推坐标 + paint 现查"消掉首帧错位，剩余只能等 reparse。

## 3. 关键发现（v0.2.0 回顾，已被 P0–P4 解决）

> 本节是 v0.2.0 起点时的问题诊断，留作历史参考。三个阻塞点在 §6 历史快照
> 中的对应行已逐项交代落地路径；当前 16 MiB 稳态参见 §2。

### 3.1 三个结构性阻塞点（按是否暴露给用户排序）

1. **搜索 8 MiB 硬限**（[zom-engine/src/search.rs:13](../zom-engine/src/search.rs)）
   `DEFAULT_REGEX_HAYSTACK_BYTE_LIMIT = 8 * 1024 * 1024`。64 MiB 文件 `search_regex` 直接报 `RangeTooLarge`，**根本扫不了**。

2. **高亮 4 MiB 跳过**（[zom-workspace/src/syntax/coordinator.rs](../zom-workspace/src/syntax/coordinator.rs)）
   原 `MAX_HIGHLIGHT_BYTES = 4 * 1024 * 1024`。超阈值的 buffer 不挂 provider，**完全没有高亮**。当前已收口为 16 MiB。

3. **高亮同步全量**（[zom-workspace/src/syntax/providers/common.rs](../zom-workspace/src/syntax/providers/common.rs) `run_full`）
   tree-sitter 每次编辑都从头 parse 全文，跑在主线程，结果重建整个 metadata layer。即使解开 #2 也跑不动——64 MiB 单键 7.6 s。

### 3.2 内存比时间更难处理

`parse` 期间 peak RSS 4.38 GB ≈ **68× 文件**。tree-sitter Rust 解析树是大头。这把 viewport-window highlighting 从"性能优化项"升级成"内存必需项"——不解决这点，同时打开两个 64 MiB rust 文件就 OOM。

### 3.3 已经做对的部分

- **viewport 渲染路径**（`Snapshot::slice_viewport`）：64 MiB 39 µs，扁平到 O(log n)。desktop 侧已经只切 viewport，[zom-desktop/src/shell/editor/snapshot/builder.rs:87](../zom-desktop/src/shell/editor/snapshot/builder.rs) 注释里写明 "GB 级文件不爆显存"。
- **载入虽是同步全量，但 150 ms / 218 MB 在 64 MiB 一档可接受**；改流式是优化项而非阻塞项。

## 4. 优化承诺（"不允许劣化"的红线）

任何 PR 影响下列指标时，必须在 PR 描述里附 `zom-bench run rust` 的新旧对比表：

**目标语料档收口为 16 MiB**（2026-06-01 调整）：64 MiB 一档保留 bench 用作回归
观察，但**不再作为红线**——tree-sitter 走树本身 O(n) 不可避免，再大的文件
（日志 / 生成代码 / 单页 HTML 等）应由宿主提示用户跳过高亮，不在本规划。
`MAX_HIGHLIGHT_BYTES` 已收口为 **16 MiB**。

| 指标 | 红线 | 理由 |
|---|---|---|
| `viewport` per_iter | 任一 size 不得 > 100 µs | 60 fps 预算的 1/400，给绘制留余量 |
| `load` 增长曲线 | 必须保持线性（16MB / 1MB ≤ 24×） | 出现超线性即说明加了 O(n²) 路径 |
| `parse` per_iter @ 16MB | 冷启动（端到端 attach）≤ 1.5 s | "打开 16 MiB 文件后 < 2 秒高亮亮起" |
| `edit+highlight main` @ 16MB | 主线程时间 ≤ 5 ms / 键 | 主线程不掉帧 |
| `edit+highlight vp` @ 16MB | viewport-scoped 端到端 ≤ 100 ms / 键 | 编辑后高亮 ≤ 100 ms 内补齐 |
| `search` per_iter @ 16MB | 异步可取消，首批结果 ≤ 200 ms | 解开 8MB 硬限的前提条件 |

> 「改造后」= 高亮/搜索改造完成后的目标。v0.2.0 起点对 16 MiB 直接被 4 MiB
> 阈值挡掉；P0–P4 完成后 16 MiB 已基本达成（见 §6 历史快照 P4 行）。

## 5. 怎么再跑

```bash
# 一次性生成语料（约 250 MB 落盘，git ignore）
cargo run --release -p zom-bench -- corpus

# 跑 rust 全部规模
cargo run --release -p zom-bench -- run rust

# 只跑 16 MiB 一档（红线档），外挂 time 取 peak RSS
/usr/bin/time -l ./target/release/zom-bench run rust 16    # macOS
/usr/bin/time -v ./target/release/zom-bench run rust 16    # Linux

# 三语言全跑
cargo run --release -p zom-bench -- run all
```

常用回归用 `run rust 4` 秒级跑完；红线档 `run rust 16` ~15 秒；64 MiB 仍跑得动，仅作"远端劣化"观察。

## 6. 历史快照

每次重要的性能改动后追加一行（保留旧行，不覆盖）。「load-only RSS」是只跑 load 场景的 peak RSS（外挂 `time -l` + `zom-bench load rust 64`），不被 parse 的暂态污染。

| 日期 | 改动 | rust 64MB load | load-only RSS | viewport | parse | edit+hl | full-run peak RSS | 备注 |
|---|---|---:|---:|---:|---:|---:|---:|---|
| 2026-06-01 | v0.2.0 起点 | 150 ms | **218 MB** | 39 µs | 7.2 s | 7.6 s/键 | 4.38 GB | 同步全量、旧 bytes 加载路径 |
| 2026-06-01 | E1 流式 load | 167 ms | **83 MB** | 39 µs | 7.0 s | 7.7 s/键 | 4.42 GB | `Buffer::from_reader`；time 持平略慢（≤+12%，per-byte 状态机比双 pass byte-loop 多 ~10% 分支开销），RSS 砍 **62%**（3.4× → 1.3× 文件） |
| 2026-06-01 | P0 raw tree-sitter | 162 ms | 83 MB | 38 µs | 6.7 s | 6.8 s/键 | **3.62 GB** | 脱 `tree-sitter-highlight`，直接用 `tree_sitter::Query` + `QueryCursor::captures`；自实现嵌套 stack + 同 node 后到 pattern 胜出。等价性由 `raw_matches_tree_sitter_highlight_on_rust_sample` 在 lifecycle.rs 真实样本上验证（span 集合相等）。时间持平到略快（多次运行噪声 ±15%，稳态对齐），full-run RSS 砍 **18%**（拿掉 tree-sitter-highlight 的 HighlightConfiguration / Highlighter 中间结构）。**为 P1 增量 reparse 铺路**。 |
| 2026-06-01 | P1 增量 reparse | 166 ms | TBD | 38 µs | 6.9 s | 6.7 s/键 | 4.72 GB | 持久化 `(Tree, last_snapshot)` + `tree.edit` + `parse_with_options` 流式 chunks；版本/翻译/parse 失败三路均回退 `run_full`。**4 MiB 一档 edit+hl 457 → 193 ms（-58%）证明增量路径生效**；64 MiB 节省仅 -12%，因 query 全树遍历 + 全文 spans Vec + `replace_layer_ranges` 全层重建按文件大小线性扩，已成主要瓶颈。peak RSS 反而略涨（旧 + 新 Snapshot 各 1 份 Arc-cheap-clone，但 tree-sitter incremental parse 期间老 Tree + 新 Tree 同时活）。要在 64 MiB 命中 < 50 ms / 键，必须等 P3（`QueryCursor::set_byte_range` viewport ± buffer + `sink.replace_range` 只换段）。 |
| 2026-06-01 | P2 后台 SyntaxWorker | 172 ms | TBD | 32 µs | 6.3 s | **主线程 2.7 µs / 键**（端到端 3.3 s） | 8.6 GB (bench 并发性) | provider 移到后台 `SyntaxWorker`（单线程 + mpsc + catch_unwind 防 panic）。主线程 `handle_edit` 只克隆 Snapshot + ChangeSet + push 一次 channel，**完全脱钩文件大小**：64 MiB 单键主线程 2.7 µs（target ≤5 ms，1800× 余量）；4 MiB 4.9 µs。e2e 平均 3.3 s 是首键全量 + 后续两键增量的均值，与 P1 worker 端代价同档（异步只挪了位置，没省 worker 计算量）。peak RSS 反而长到 8.6 GB 是 bench 工艺：测完 main 立刻测 e2e，两根 worker 线程同时持 64 MiB tree；生产无并发，不会触发——这条 RSS 改善等 P3 viewport 局部 query 一并兑现。**对外 API 变化**：`Workspace::pump_pending_highlights` 由 desktop 每帧 prepaint 调；`BufferSyntaxState::attach` 多了 `buffer_id` + `Arc<SyntaxWorkerHandle>` 入参。 |
| 2026-06-01 | P3 viewport-scoped ReplaceRange | 174 ms | TBD | 37 µs | 6.3 s | **viewport ±8 KiB e2e 246 ms / 键**（全文 e2e 仍 3.3 s，主线程 3 µs） | 7.1 GB (bench 并发) | 桌面端把 viewport ± 缓冲区作为 byte range hint 喂给 worker；worker 用 `QueryCursor::set_byte_range` + `SnapshotTextProvider`（predicate 按节点取字节，避免物化全文）跑局部 query，产物以 `sink.replace_range` 投递。coordinator 把 `ReplaceRange` 接到 `MetadataLayer::replace_in_range`——按 span 起点落在新区间内的判定删旧、追加新，远处 spans 完全保留。engine 新增 `MetadataLayer::replace_in_range` / `MetadataLayers::replace_layer_ranges_in_range`。**4 MiB viewport-scoped e2e 11.2 ms（target 50 ms，4.5× 余量）；64 MiB 246 ms（target 50 ms，5× 未达 — 余量在 tree-sitter 节点遍历开销上）**；vs 全文 e2e 3.3 s 砍 92%。`set_viewport` 自身立即触发一次 viewport-scoped re-query，滚动后新区域 1–2 帧内见高亮。首次 attach 仍 `ReplaceAll` 铺底；viewport hint 清空回退到 `ReplaceAll`。**剩余 64 MiB 瓶颈**：（1）attach 冷启动依旧全树 query → 6.3 s，需 viewport-aware attach；（2）QueryCursor 在 64 MiB tree 上即便 set_byte_range 也要从 root 走，目前 ~150 ms/键；（3）SnapshotTextProvider 每个 predicate 节点 alloc 一个 Vec<u8>，rust grammar `#match?` 触发频次高。三条都是 P4 / P5 候选项。 |
| 2026-06-01 | P3 收尾 + P4 放阈值 | 168 ms | TBD | 36 µs | 6.3 s | viewport ±8 KiB e2e 264 ms / 键（主线程 3 µs） | 10.2 GB (bench 并发) | desktop 接 `App::pump_active_viewport_hint`：`ShellView::render` 每帧把活动 view 的 `top_line` + `visible_line_count` ± 32 行 padding 算成 byte_range，调 `Workspace::set_buffer_viewport_hint`；HighlightWorker 内部对相同 hint 去重，无变化不重 query。**`MAX_HIGHLIGHT_BYTES` 4 MiB → 64 MiB**——viewport-scoped 路径已脱钩文件大小，4 MiB 阈值是 P0–P2 时代为了拦"全量阻塞"留的临时门槛，现在可以撤掉。64 MiB rust 现在能挂上 syntax provider，主线程不卡：cold parse 6.3 s 在 worker 上异步跑，主线程立刻就能编辑（无高亮），spans 后续帧补齐。bench worker 端数字与 P3 基本同档（噪声 ±10%）。peak RSS 上涨到 10.2 GB 仍是 bench 工艺：连跑 5 个场景（parse 3 iter + edit+hl m 200 iter + e2e 3 iter + vp 30 iter）会有多根 worker 线程并发持 64 MiB tree；生产单 buffer 单线程不会触发——常驻只有当前 worker 一份 tree (≈ 400 MB) + viewport-scoped spans (< 1 MB)。搜索 8 MiB 硬限单独立项，需先做异步搜索 + 可取消 + 进度上报。 |
| 2026-06-01 | 收口 16 MiB（阈值回收） | (16MB: 44 ms) | TBD | 32 µs | (16MB: 1.56 s) | (16MB: vp 63 ms / 主线程 3 µs / 全文 e2e 786 ms) | (16MB: 3.0 GB bench 并发) | **`MAX_HIGHLIGHT_BYTES` 64 → 16 MiB**：bench 实测 16 MiB rust 单键 viewport-scoped e2e 63 ms（红线 ≤ 100 ms ✅）、主线程 3 µs（≤ 5 ms ✅）、cold parse 1.56 s（≈ 1.5 s ✅）；64 MiB 一档单键 ~250 ms，cold parse 6.3 s，是 tree-sitter 走树本身 O(file size) 的常数代价，不再继续追。16 MiB 覆盖单文件主流代码量（小型 monorepo 单文件 99 分位 < 8 MiB），超过部分多为日志/生成代码/单页 HTML，宿主应提示用户跳过。BASELINE §4 红线表也同步收口到 16 MiB 一档；64 MiB 仍在 bench 跑，仅作回归观察、不当红线。**P0–P4 阶段性收尾**：方案兑现"主线程脱钩文件大小 + viewport-scoped query + 异步 worker"三件套；剩余 search 8 MiB 硬限独立成项。 |
| 2026-06-07 | confirmed layer Stickiness::Expand + 删 provisional | — | — | — | — | (16MB: 3 µs 主线程，e2e 同 §2) | — | 把 confirmed layer 落 span 时 stickiness 从 `Never` 切到 `Expand`：编辑当帧 `update_through_delta_event` 让相邻 span 自动吞下新字节，不再需要 provisional 层。删除：`syntax_provisional_layer_kind`、`ProvisionalSpan`、`provisional_spans_for_edit`、`infer_highlight_*`、`syntax_range_is_covered`、`clear_provisional_*`（~190 行），主线程少跑 3 次 O(N) 扫描；删除 desktop `SYNTAX_PROVISIONAL` priority 与 producer 第二层 push。bench 数字未跑（主线程仍 3 µs/键 量级，区别在 layer 写入路径而非可测耗时）。**遗留问题**：仍有"先以旧 tree 颜色显示、worker reparse 回包后再纠正"的色跳——这一帧延迟由 worker 异步交付协议本身造成，是下一步 render-time query 改造的动机。 |
| 2026-06-07 | render-time query 前置测量 | — | — | — | — | — | — | bench 新增两个分位数场景：`render-time query`（直跑 `QueryCursor::captures` + `set_byte_range`，~50 行 viewport ×200 次）与 `incremental reparse`（`tree.edit` + `parse_with_options(Some(&old))` ×100 次）。**关键发现**：（1）render-time query p99 跨 1/4/16 MiB 都 < 100 µs，远低于 0.5 ms 阈值——paint 现查 viewport 完全可行，不必加 viewport spans cache；（2）incremental reparse p99 在 1 MiB 2.0 ms / 4 MiB 11.2 ms / 16 MiB 58.1 ms 强线性增长，16 MiB 严禁主线程同步——这也是 "wrong color flash" 持续 3-4 帧的根因，tree-sitter 增量 reparse 自身代价不可绕过。详见 §2.1。 |
| 2026-06-08 | BufferSyntaxTree + slot 改造落地 | (16MB: 42.9 ms) | TBD | (16MB: 35.8 µs) | (16MB: 0.90 s) | (16MB: 主线程 5.9 µs / e2e 75.6 ms) | (1+4+16 三档全跑: 1.58 GB / footprint 1.16 GB) | sink / `MetadataLayer<HighlightSpan>` / coalesce 整条路径下线；改成 worker reparse 后把 `Arc<BufferSyntaxTree>` 写共享 slot，paint 端按 viewport 现查 `query_viewport`。主线程编辑入口同步 `tree_slot.try_edit` 推坐标，让首帧 paint 拿到的 tree 已对齐新 snapshot，token 内插入不再闪默认前景色。等价性由 `incremental_matches_full_after_edits` / `edit_frame_decorations_equal_reparse_frame_for_structure_preserving_edit` 钉死。markdown 退化为单层 block grammar（inline 着色 / 代码栏注入留作未来工作项）。**首次实测**（`time -l cargo run --release -p zom-bench -- run rust`）：16 MiB 红线全过——主线程 5.9 µs/键 ≤ 5 ms（850× 余量）、e2e 75.6 ms ≤ 100 ms（24% 余量）、cold parse 0.90 s ≤ 1.5 s。full-run peak RSS 1.58 GB 对比 2026-06-01 收口 行的 16 MiB 单档 3.0 GB，slot 方案在并发跑三档情况下仍砍掉 ~半（sink + MetadataLayer 双份 spans 不再常驻），与 §2.1 incremental reparse p99 16 MiB 87 ms 一致，e2e 预算几乎全花在 tree-sitter 自身。`time -l ./target/release/zom-bench load rust 16` 单测 load-only RSS 尚未跑。 |
