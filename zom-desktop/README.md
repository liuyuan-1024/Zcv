# zom-desktop

`zom-desktop` 是 zom 的桌面入口 crate：GPUI 外壳 + 组合根。

## 定位

`zom-desktop` 是二进制入口，也是把其它 crate 组装起来的**组合根**。

它内部分两层：

- `shell` —— GPUI 渲染、窗口、字体度量、主题、像素布局、OS 事件源、输入解码（OS 原始按键 → 归一化 `KeyChord`）、OS 级 IME。天然绑死平台。
- `app` —— 事件循环、输入路由、handler 注册、wiring。编排逻辑。

作为组合根，它认识所有人 —— 这正是它的职责。这与「可复用的派发基础设施认识所有人」（错误）相区别。

`zom-command` 与 `zom-ai` 在这里相遇：两者无依赖边，由组合根注册 `ai.*` handler、把 AI 意图翻译成命令。

纪律：`app` 若以后长出想无头测试的编排逻辑（多窗口、会话恢复、布局管理），*那时*才抽 `zom-app`，不预建。

## 核心职责

- 建 `CommandRegistry` / `Workspace` / `ViewSet` / `Keymap`，调 `register_builtin_editor_commands`，注册 `ai.*` handler。
- 事件循环：`KeyChord → keymap 解析 → 命令队列 → 执行器 → 重绘`。
- 拥有 async 运行时。
- 配置加载（keymap、主题、设置）—— 它是二进制，知道文件系统布局。
- IME：OS 事件 → `shell` 解码 → 活动 view 的 `CompositionState`；start / commit 走命令，update 走直接通道。

## 依赖

```text
zom-desktop → zom-engine / zom-workspace / zom-view / zom-command / zom-ai
```

依赖全部 5 个宿主 crate，含直接依赖 `zom-engine`（渲染要用 `Snapshot` / `ViewportSlice`）。

`gpui` 与 async 运行时依赖在 `shell` / `app` 落地实现时接入，骨架阶段不预拉。

## 结构概览

```text
src/main.rs    二进制入口，声明 shell / app 模块
src/shell/     GPUI 外壳（占位）
src/app/       组合根与编排，App
```

## 相关文档

- `../AGENTS.md`：workspace 全局协作规则。
- `../TODO.md`：宿主层开发规划，本 crate 对应阶段 P2。

## 状态

骨架阶段：`shell` / `app` 模块结构已定，`App` 组合根雏形已铺。事件循环、GPUI 接入、async 运行时接入留待 `TODO.md` P2。
