# zcv-editor

`zcv-editor` 是可嵌入的文本编辑组件。它把文本与语言状态投影成可交互的编辑视图，并负责选择、滚动、输入法和编辑器交互状态。

公共入口是 [`src/editor.rs`](src/editor.rs)。crate 对外提供 `Editor`、编辑事件、滚动锚点和差异块委托；内部显示、输入与渲染细节保持私有。

## 数据流与所有权

```text
zcv-text::Buffer
        ↓
zcv-language::LanguageBuffer
        ↓
zcv-multi-buffer::MultiBuffer
        ↓
zcv-editor::Editor
        ↓
DisplayMap
        ↓
EditorElement
```

- `Buffer` 是文本内容的权威数据源。
- `LanguageBuffer` 持有 Tree-sitter 语言与语法状态。
- `MultiBuffer` 负责把一个或多个缓冲区组织成编辑器消费的文档视图。
- `Editor` 持有选择、选择历史、滚动、输入法组合、焦点、搜索、折叠和编辑模式等交互状态。
- `DisplayMap` 从文档与编辑器配置派生可见行、折叠、软换行和装饰投影；它不是第二份文本模型。
- `EditorElement` 连接每帧布局、绘制和输入命中，不长期持有文档事实。

状态应由最接近其生命周期的层维护。不要在 UI、显示投影或调用方中复制可写的文本、选择或滚动状态。

## 创建方式

`Editor` 提供三类入口：

- `Editor::single_line`：单行输入场景。
- `Editor::auto_height`：随内容调整高度的输入场景。
- `Editor::for_multi_buffer`：面向完整文档或组合文档的编辑场景。

`zcv-editor::init` 完成两项应用级接入：

1. 向 `zcv-ui` 提供类型擦除的单行编辑器工厂，使设计系统无需反向依赖编辑器。
2. 注册工作区 `ItemProvider`，使文件能够由工作区恢复和打开。

调用应用必须在创建依赖这些能力的 UI 或恢复工作区之前执行初始化。

## 边界规则

- 文本编辑通过底层模型的事务完成，不在视图层维护影子内容。
- 语法能力放在 `zcv-language`，当前不建立 LSP 抽象。
- 只服务编辑器的显示、选择、滚动和输入实现保留在本 crate 内。
- 工作区标签、Dock 和持久化属于 `zcv-workspace`；编辑器只实现其 `Item` 协议。
- 可复用的视觉原语属于 `zcv-ui`；编辑器专用控件留在编辑器附近。
- 公共 API 只暴露真实跨 crate 消费方需要的协议与类型。

## 修改检查

修改编辑器时，至少确认：

1. 文本、语法、组合文档、显示投影与交互状态的所有者没有混淆。
2. 字节偏移、字符边界、视觉行和缓冲区位置之间的转换发生在明确边界。
3. 键盘焦点上下文与视觉焦点分别处理，子元素焦点不会意外冒充编辑器本体焦点。
4. 新行为有对应的可观察测试，过时路径与状态一并删除。

## 验证

```bash
cargo check -p zcv-editor
cargo test -p zcv-editor <相关测试过滤条件>
```

测试位于各模块的 `test` 目录或测试模块中。选择能直接覆盖改动行为的最小测试集合。
