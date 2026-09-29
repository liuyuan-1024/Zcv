# zcv-picker

`zcv-picker` 提供通用“搜索 + 选择”浮层原语：`Picker` 负责搜索输入、过滤与列表导航，`PickerHost` 负责浮层开合与互斥。

公共入口是 [`src/picker.rs`](src/picker.rs)。

## 职责

- 承载 `PickerDelegate`（业务数据与匹配逻辑）。
- 持有单行搜索输入、查询文本与列表滚动状态。
- 由 `PickerHost` 管理浮层显隐、焦点与“当前活动浮层”互斥。

本 crate 不拥有业务数据；项目选择、最近项目、分支选择等由宿主提供 delegate。

## 所有权

- `Picker<D>` 拥有 delegate、搜索输入、查询与列表状态。
- 搜索输入由 `zcv_editor::init` 注入的 `EDITOR_FACTORY` 创建，是查询输入的权威。
- `PickerHost` 拥有浮层开合状态；宿主（如工作区）拥有 host 的生命周期。

## 不变量

- 编辑器工厂缺失是装配顺序错误，必须显式失败，不能静默降级为无搜索框。
- delegate 的匹配查询由 `Picker` 驱动；宿主清空搜索应调用 `Picker::set_query`，同步输入、匹配结果和选中项定位。
- 匹配结果由数据源主动变更后，调用 `Picker::matches_updated` 重建行测量并保留滚动位置；不能只在渲染时比较行数，同样数量的结果也可能已经变化。
- 结果区使用纵向弹性布局，虚拟列表的视口等于其剩余可见空间。搜索框与 footer 不参与结果区滚动；浮层高度不超过窗口。
- 可变行高结果在选择器的固定宽度下建立完整高度索引，由 `ListState` 缓存并随匹配结果失效；绘制仍只消费可见行。离屏项不能以零高度参与滚动定位。
- 浮层互斥由 host 明确维护。

## 验证

```bash
cargo check -p zcv-picker
cargo test -p zcv-picker
```
