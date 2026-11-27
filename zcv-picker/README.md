# zcv-picker

`zcv-picker` 提供通用“搜索 + 选择”能力：`Picker` 负责搜索输入、过滤与列表导航，浮层和焦点生命周期由 `zcv-ui::PopoverHost` 承载。

公共入口是 [`src/picker.rs`](src/picker.rs)。

## 职责

- 承载 `PickerDelegate`（业务数据与匹配逻辑）。
- 持有单行搜索输入、查询文本与列表滚动状态。
- `toggle_picker` 把选择器的焦点范围与搜索输入目标交给 `PopoverHost`，供宿主打开或关闭选择器。

本 crate 不拥有业务数据；项目选择、最近项目、分支选择等由宿主提供 delegate。

## 所有权

- `Picker<D>` 拥有 delegate、唯一搜索输入、输入订阅与列表滚动状态。候选结果、选中索引及业务任务由 delegate 持有，宿主不维护另一份。
- 搜索输入由 `zcv_editor::init` 注入的 `EDITOR_FACTORY` 创建，是查询输入的权威。
- `PopoverHost` 拥有浮层开合状态；宿主（如工作区）拥有 host 的生命周期。

## 呈现与业务更新

- 普通选择器通过 `Render` 整体呈现。常驻输入场景通过 `render_input` 和 `render_results` 分别挂载输入与结果；两者消费同一个 Picker，输入编辑器每帧只挂载一次，键盘与鼠标共用同一份选中项和列表状态。
- `QuickAccess` 只负责顶栏和结果浮层布局。其 delegate 拥有文件扫描、文件与命令混合排序、行列模式及扫描任务；扫描结束后调用 `Picker::refresh`，从当前输入重建结果，不安装旧查询的结果副本。
- `PickerDelegate::update_matches` 接收 Picker 上下文，业务可在自身生命周期内持有异步任务；Picker 不为所有选择器预建异步调度状态。
- 取消由 Picker 发出 `DismissEvent`；确认交给 delegate 决定是否发出该事件。宿主订阅事件后关闭 `PopoverHost` 并归还焦点；无候选或确认未完成时可以保持打开。命令类确认先发出关闭事件，再延迟分发命令。
- 分离呈现的宿主观察 Picker 的通知以重新布局，调整宽度通过 `set_width` 使旧行高估计失效，不复制输入、匹配结果或滚动状态。
- 空结果提示由 delegate 的 `render_empty` 提供；最近项目底部的操作区由 `render_footer` 提供。

## 不变量

- 编辑器工厂缺失是装配顺序错误，必须显式失败，不能静默降级为无搜索框。
- delegate 的匹配查询由 `Picker` 驱动；宿主清空搜索应调用 `Picker::set_query`，同步输入、匹配结果和选中项定位。
- 匹配结果由数据源主动变更后，调用 `Picker::matches_updated` 重建行测量并保留滚动位置；不能只在渲染时比较行数，同样数量的结果也可能已经变化。
- 结果区使用纵向弹性布局，虚拟列表的视口等于其剩余可见空间。搜索框与 footer 不参与结果区滚动；浮层高度不超过窗口。
- 可变行高结果由 `ListState` 按需测量可见行；首帧用实测可见行高度估计离屏项，匹配结果变化时重建估计，进入视口后以实际高度替换。
- 浮层互斥由 host 明确维护。

## 验证

```bash
cargo check -p zcv-picker
cargo test -p zcv-picker
```
