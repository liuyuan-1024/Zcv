# zom-desktop

`zom-desktop` 是 zom 的桌面入口 crate：GPUI 外壳 + 组合根。

## 定位

`zom-desktop` 是二进制入口，也是把其它 crate 组装起来的组合根。

它内部分两层：

- `shell` —— GPUI 渲染、窗口、字体度量、主题、像素布局、OS 事件源、输入解码（OS 原始按键 → 归一化 `KeyChord`）、OS 级 IME。天然绑死平台。
- `app` —— 事件循环、输入路由、handler 注册、wiring。编排逻辑。

作为组合根，它认识所有运行期边界 —— 这正是它的职责。这与「可复用的派发基础设施认识所有人」相区别。

纪律：`app` 若以后长出想无头测试的编排逻辑（多窗口、会话恢复、布局管理），*那时*才抽 `zom-app`，不预建。

## 核心职责

- 建 `CommandRegistry` / `Workspace` / `ViewSet` / `Keymap`，安装 `zom-command::commands::*` catalog。
- 事件循环：`KeyChord → keymap 解析 → 命令队列 → 执行器 → 重绘`。
- 异步工作统一走 GPUI 执行器或 crate 内现有后台 worker，不在组合根预拉独立 runtime。
- 配置加载（keymap、主题、设置）—— 它是二进制，知道文件系统布局；当前先使用内建默认值。
- IME：OS 事件 → `shell` 解码 → 活动 view 的 `CompositionState`；start / commit 走命令，update 走直接通道。

## 依赖

```text
zom-desktop → zom-engine / zom-workspace / zom-view / zom-command
```

依赖全部宿主 crate 中的 `zom-engine` / `zom-workspace` / `zom-view` / `zom-command`。

`gpui` 已在 `shell` 落地，用于窗口启动、资源加载和基础外壳渲染。async 运行时仍遵循桌面端设计手册约定，优先使用 GPUI 执行器，不预拉独立 runtime。

## 结构概览

```text
src/main.rs                  二进制入口，声明 shell / app 模块
src/app.rs                   App 状态、命令派发、keymap 解析、IME 桥接
src/focus.rs                 应用语义焦点模型
src/shell/boot.rs            GPUI 应用启动与首窗口装配
src/shell/view/              ShellView、渲染根、焦点投影与 HostEffect 落地
src/shell/editor/            嵌入式文本编辑目标、输入、快照与编辑区渲染
src/shell/features/          L3 功能切片：面板、项目选择器、设置、语言服务器入口
src/shell/platform/          平台差异收纳层
src/shell/shared/            资源、键盘、主题 token 与 L2 视觉原语
src/shell/surfaces/          浮层 surface 的定位、生命周期与渲染承载
src/shell/workbench/         顶栏、底栏、dock、panel host、编辑区布局状态
assets/                      内置图标、字体与默认主题
```

## 相关文档

- `../agents/global.md`、`../agents/project.md`：workspace 全局规则与项目规则。
- `../TODO.md`：宿主层开发规划。

## 文档维护

本 README 只维护稳定边界、核心职责、依赖关系与目录概览；阶段规划与设计决策放入 `../TODO.md` 和 `docs/` 文档。
