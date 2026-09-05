# zcv-workspace

`zcv-workspace` 提供编辑器窗口的通用框架：`Item`、`Pane`、`Dock`、`Panel`、工具条、状态栏、持久化和 `Workspace` 装配。

公共入口是 [`src/workspace.rs`](src/workspace.rs)。本 crate 不依赖具体编辑器或预览格式；具体能力通过协议与注册入口接入。

## 所有权

- `Workspace` 是窗口级根实体，拥有当前 `Project`、中心 `Pane`、三个 `Dock`、状态栏、Toast 层和布局持久化任务。
- `Pane` 拥有中心区域的 `ItemHandle` 集合、活动项与标签生命周期。
- 每个 `Item` 自己拥有文档或视图状态，通过 `ItemHandle` 在 `Pane` 中类型擦除。
- 每个 `Panel` 是独立 `Entity`，自己拥有面板状态与焦点；`Dock` 只组织异构面板、开合和尺寸。
- 宿主 `zcv` 注入顶栏、面板、产品级订阅与命令处理，不把这些具体模块变成工作区框架的依赖。

同一状态只由一层写入。`Workspace` 不复制 `Item` 内容，`Dock` 不复制 `Panel` 业务状态，宿主也不维护第二份布局事实。

## 接入协议

### Item

实现 `Item` 以进入中心 `Pane`。协议覆盖标签标识、焦点、保存、关闭、导航以及可选能力。搜索、预览和 `MultiBuffer` 访问通过可选接口暴露，而不是让工作区了解具体类型。

### Panel

实现 `Panel` 以进入某个 `Dock`。面板负责自己的名称、焦点、事件和可持久化状态；工作区负责摆放与恢复。

### Provider

- `ItemProvider` 根据路径创建或恢复具体 `Item`。
- `PreviewProvider` 根据文件能力创建具体预览。

Provider 是框架反转点。注册应由具体功能 crate 的初始化入口完成；不要让 `zcv-workspace` 依赖所有实现方。

## 生命周期

1. 宿主创建空工作区或绑定本地 `Project` 的工作区。
2. `Workspace` 创建 `Pane`、Dock、状态栏和持久化设施。
3. 宿主注册面板、顶栏、订阅和产品级命令。
4. 首帧后，在 Provider 与面板就绪时恢复标签和布局。
5. 标签、Dock 或面板状态变化后，工作区节流保存布局。
6. 替换窗口根或退出前，宿主冲刷待保存的布局状态。

## 边界规则

- 工作区只定义通用容器和协议，不包含具体编辑、终端、项目树或预览业务。
- 单一面板或单一 Item 的辅助代码与其实现放在一起，不提升到框架公共层。
- 全局可达的命令由窗口根挂载，但命令行为仍由真实状态所有者实现。窗口最小化与最大化由 `Workspace` 处理，`TopBar` 的私有窗口按钮只负责渲染与分发 action。
- 持久化保存稳定身份和可恢复状态，不序列化运行时 `Entity` 或派生视图。

## 验证

```bash
cargo check -p zcv-workspace
cargo test -p zcv-workspace <相关测试过滤条件>
```
