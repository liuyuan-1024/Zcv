# zcv-theme

`zcv-theme` 提供界面视觉 token：语义色、语法高亮表、排版、间距与文件图标，以及主题注册表和 `ThemeChoice` 入口。

公共入口是 [`src/theme.rs`](src/theme.rs)。主题数据（语义色 + 语法高亮）由 `theme_data` 注册表统一持有，新增主题只需添加 TOML 并在注册表登记。

## 职责

- 解析内置主题文件为运行时可读取的语义色表与语法样式表。
- 提供字号、字体栈与行高推导（打字排版）。
- 提供跨组件共享的间距 token 与文件图标映射。

本 crate 不负责工作区布局、组件渲染或用户设置存储；字号的基础快照由设置层写入，临时覆盖由工作区持有。

## 所有权

- `theme_data` 是主题数据的唯一解析与持有者。
- 当前语义色、语法表与基础排版分别存放在 **App 级 global** 中，随主题/设置整体替换；不保留任何进程级可变主题静态。
- `color::current(cx)`、`syntax::style_table(names, cx)`、`typography::current(cx)` 是各自的读取入口。
- 工作区的临时排版覆盖属于 `zcv-workspace::Workspace`，不回写基础 global；渲染经窗口级入口读取生效快照。

## 不变量

- 同一项视觉事实只有一个权威来源：主题色/语法表来自 `theme_data`，排版基准来自内置设置文件。
- 主题子状态不跨 App 泄漏：每个 App 的主题互不影响。
- 行盒必须容纳字形墨迹（行高 ⊇ 墨迹），溢出裁剪容器不裁字。
- 未注入主题时读取回退到首个内置主题，保证无主题上下文仍可渲染。

## 验证

```bash
cargo check -p zcv-theme
cargo test -p zcv-theme
```
