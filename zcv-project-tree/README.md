# zcv-project-tree

`zcv-project-tree` 实现项目文件树面板 `ProjectTreePanel`：行渲染、选择、拖拽移动、行内重命名与文件操作动作转发。

公共入口是 [`src/project_tree.rs`](src/project_tree.rs)。模块按职责拆分：`render`（行渲染）、`drag`（拖拽）、`editing`（行内编辑）、`transfer`（文件移动/复制）、`execute`（动作执行）、`actions`（命令）。

## 职责

- 订阅 `Project` 与文件监视事件并刷新可见行。
- 把树行渲染为 `zcv-ui` 的通用树行几何。
- 把用户操作转换为项目文件操作并处理错误反馈。

## 所有权

- 面板拥有自身的选中、展开、编辑与拖拽状态。
- 文件系统与文件状态事实由 `zcv_project::Project` 拥有，面板只做派生投影。

## 不变量

- 树行行高/缩进来自 `zcv-ui::TreeRowFrame`，按窗口 `rem_size` 计算，保证行与命中测试一致。
- 面板不复制项目文件状态；刷新由事件驱动。
- 测试位于 `src/test/`，通过面板公开行为验证。

## 验证

```bash
cargo check -p zcv-project-tree
cargo test -p zcv-project-tree
```
