# zom-desktop 智能体约束

## app / shell 边界

- `app/` 是应用域运行时：命令装配、工作区、配置、文本路由、后台拍点、持久化。它只把输入收敛为 `Invocation` 并产出 `HostEffect`。
- `app/` 不得接触 GPUI、窗口、像素、焦点 handle、字体、布局，也不得引用 shell 的具体类型或知道 UI 长什么样。
- `shell/` 是 GPUI 外壳：窗口、视觉、平台输入、IME、鼠标键盘事件、`HostEffect` 到 UI 状态的翻译。
- `shell -> app` 的白名单只有 `use crate::app::App`。不得 `use crate::app::*` 或访问 app 子模块、store、runtime、router、pump。
- `app -> shell` 的接入必须走顶层共享协议或端口，例如 `PostEditObserver`、`FramePump`、`TextTargetOwner`。shell feature 可以实现这些 trait 并在装配期注册给 `App`，但 `App` 不能保存 `FileTreeModel` 这类 shell 具体类型。
- 不要把所有组件动作都绕到 `App`。组件自己的 runtime 状态由组件自己处理；只有"组件动作 + workspace/session 或其他应用域副作用"才进入 `App` 做组合，并通过窄端口产出声明式 `HostEffect`。例如文件树选择、展开/折叠、开始/取消重命名属于 `FileTreeRuntime`；打开文件、提交新建/重命名、确认删除、paste 这类会改 workspace/session 的动作才走 `App`。
- 顶层共享词汇表优先保持在这四个文件：`config.rs`（`AppConfig`）、`focus.rs`（`AppFocus`）、`workspace_session.rs`（`WorkspaceSession`）、`ports.rs`（反向端口）。新增共享词汇前先确认它不是 app 内部或 shell 内部。
- 文本路由协议支撑模块可以在顶层：`editor_text/` 放 IME 目标、文本快照、自持输入框；`editor_highlight/` 放装饰语义与 producer。它们不能解析主题、绘制、引用 shell，颜色解析留在 `shell/editor`。
- app 内部除 `App` 外默认 `pub(super)` 或私有，用编译错误阻止 shell 越界。
- 边界通信遵循 `Invocation -> App::dispatch_* -> Vec<HostEffect> -> shell 解释`，不要互相戳内部结构，也不要在 shell 里另存一份应用域状态。

更完整的说明见 `docs/桌面端设计手册.md` 的 `app / shell 边界` 与 `可见性白名单` 小节。
