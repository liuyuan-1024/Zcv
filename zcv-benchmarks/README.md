# zcv-benchmarks

固定文本与组合规模，测量文本、显示投影和编辑帧的 CPU 成本。基准不能代替原生窗口的帧率验证。

## 组合文档滚动

`editor_display` 的 `editor/diff_scroll_render_frame` 覆盖暂存、未暂存、词级差异及大新增块。GPUI 测试上下文默认关闭自动换行；`render_only` 只计绘制调用，`input_to_frame` 计滚轮输入到测试绘制。连续滚动交替改变方向，避免长期钳制在文首或文尾；`jump_to_warmed_region` 是反复跳入已经访问的区域，不能解释为首次进入的冷成本。

```bash
cargo bench --offline -p zcv-benchmarks --bench editor_display -- 'editor/diff_scroll_render_frame/word_diff/unstaged/16/input_to_frame' --quick
```

软换行场景使用 Editor 模块内的手动探针。夹具分别建立暂存只读和未暂存可编辑的真实 diff，展开删除侧；每个文件 50 行变更，包含长行、中文与 Tab，覆盖 10 和 100 个文件。固定窗口宽度，先让后台重排收敛，再测量 240 帧交替滚动；测量同时验证滚动不推进显示版本。

```bash
cargo test --offline --release -p zcv-editor composite_soft_wrap_scroll_frame_latency_probe --lib -- --ignored --nocapture
cargo test --offline --release -p zcv-editor soft_wrap_reflow_latency_probe --lib -- --ignored --nocapture
```

第一项报告滚轮输入到测试绘制的中位数、P95 与范围；第二项单独报告宽度变化后的前台返回与重排收敛耗时。两者不能混合成一个“滚动耗时”。耗时受机器负载影响，应在没有并行构建时重复运行，按相同构建、缓存状态和场景比较。

测试文本系统与真实字体塑形、系统输入队列和 GPU 呈现不同；上述测量也未包含整个 DiffView 的宿主控件。原生 release 验收需使用同一组文件、窗口宽度与字体，分别检查连续滚动、快速大跳转、编辑后滚动和调整窗口宽度，并用主线程采样与帧时间确认。

## 正确性与生命周期

定向回归分别验证稳定 Anchor、局部块拼接、关闭软换行的行 patch、异步重排取消、滚动快照复用、批量折叠事件及高亮缓存的预算淘汰。换行器归还 GPUI 池后，字体宽度缓存由文本系统保留；释放检查应验证反复取消后池规模稳定、文档和任务输入释放，不能要求文本系统的引用计数恢复到创建池之前。
