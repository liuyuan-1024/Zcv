# zom-desktop

`zom-desktop` 是 zom 的桌面入口 crate：GPUI 外壳 + 组合根。

## 定位

`zom-desktop` 是二进制入口，也是把其它 crate 组装起来的**组合根**。

它内部分两层：

- `shell` —— GPUI 渲染、窗口、字体度量、主题、像素布局、OS 事件源、输入解码（OS 原始按键 → 归一化 `KeyChord`）、OS 级 IME。天然绑死平台。
- `app` —— 事件循环、输入路由、handler 注册、wiring。编排逻辑。

作为组合根，它认识所有人 —— 这正是它的职责。这与「可复用的派发基础设施认识所有人」（错误）相区别。

P4 接入 AI 时，`zom-command` 与 `zom-ai` 会在这里相遇：两者无依赖边，由组合根注册 `ai.*` handler、把 AI 意图翻译成命令。

纪律：`app` 若以后长出想无头测试的编排逻辑（多窗口、会话恢复、布局管理），*那时*才抽 `zom-app`，不预建。

## 核心职责

- 建 `CommandRegistry` / `Workspace` / `ViewSet` / `Keymap`，安装 `zom-command::commands::*` catalog，后续注册 `ai.*` handler。
- 事件循环：`KeyChord → keymap 解析 → 命令队列 → 执行器 → 重绘`。
- 后续需要异步任务时，拥有 async 运行时或统一使用 GPUI 执行器。
- 配置加载（keymap、主题、设置）—— 它是二进制，知道文件系统布局；当前先使用内建默认值。
- IME：OS 事件 → `shell` 解码 → 活动 view 的 `CompositionState`；start / commit 走命令，update 走直接通道。

## 依赖

```text
zom-desktop → zom-engine / zom-workspace / zom-view / zom-command
```

依赖全部宿主 crate 中的 `zom-engine` / `zom-workspace` / `zom-view` / `zom-command`；`zom-ai` 会在 P4 接入 AI 命令闭环时加入组合根。

`gpui` 已在 `shell` 落地，用于窗口启动、资源加载和基础外壳渲染。async 运行时仍遵循桌面端设计手册约定，后续直接使用 GPUI 执行器，不预拉独立 runtime。

## 结构概览

```text
src/main.rs         二进制入口，声明 shell / app 模块
src/app/mod.rs      App 状态、启动装配、编辑器状态投影
src/app/command.rs  命令派发、keymap 解析与 HostEffect 收集
src/app/ime.rs      IME 桥接与 UTF-16 / byte 坐标换算
src/shell/mod.rs    shell 门面与共享回调类型
src/shell/boot.rs   GPUI 应用启动与首窗口装配
src/shell/view.rs   ShellView、渲染根与 EntityInputHandler
src/shell/shared/keyboard.rs
                    OS 按键 → 归一化 KeyChord 字符串
src/shell/shared/   资源、element id、主题 token 与 L2 视觉原语
src/shell/features/ L3 功能切片：业务 panel、功能 overlay、PanelId 与 PanelHost
src/shell/workbench/controller.rs
                    WorkbenchController，持有每窗口 dock / panel UI 状态
src/shell/workbench/state.rs
                    WorkbenchState / DockState 等窗口布局状态
src/shell/workbench/regions/{left,bottom,right}_dock.rs
                    各 dock 自己声明固定承载的 panel
src/shell/workbench/regions/
                    L4 窗口外壳区域
```

## 相关文档

- `../AGENTS.md`：workspace 全局协作规则。
- `../TODO.md`：宿主层开发规划，本 crate 对应阶段 P2。

## 状态

P2 已形成最小编辑闭环：GPUI 外壳、embedded assets、基础布局、panel 骨架、自绘窗口控制、默认空白 buffer/view、keymap→command→engine 派发、文本显示、删除、移动、撤销、重做和 IME 最小输入路径已接入。搜索、viewport slice、fold / projection 与 AI 提案闭环继续按 `TODO.md` P3/P4 推进。
