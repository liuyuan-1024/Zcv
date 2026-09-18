# zcv-keymap

`zcv-keymap` 加载平台内置 keymap，经 GPUI action registry 解析为 `KeyBindings`，供应用注册与界面反向查询。

公共入口是 [`src/keymap.rs`](src/keymap.rs)。

## 职责

- 读取平台对应的内置 keymap 文件（支持 JSONC 行注释）。
- 把键位解析为 `gpui::KeyBinding` 并注册到 App。
- 维护“action → 显示快捷键文本”的查询，供 UI 装配层展示。

本 crate 不负责命令实现，也不定义组件。

## 所有权

- `KeyBindings` 是 App 级 global，只能由 `init(cx)` 创建并写入。
- 绑定列表字段私有，外部只能经 `display_shortcut` / `display_shortcut_named` 查询。

## 不变量

- 注册行为与界面提示同源：两者由同一次 keymap 加载产生，不维护第二份快捷键表。
- 未知或非法 action 必须使加载失败，不能静默跳过。
- 设计系统组件不直接依赖本 crate；调用方用 `display_shortcut(action, cx)` 解析文本后注入组件。

## 验证

```bash
cargo check -p zcv-keymap
cargo test -p zcv-keymap
```
