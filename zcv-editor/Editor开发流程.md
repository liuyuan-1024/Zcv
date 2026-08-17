# Editor 开发流程

> 本文规定可嵌入文本编辑器的目标架构、模块边界与开发顺序。
> Editor 在实现方法上对标 Zed，以 `zcv-engine` 为文本内核，在独立
> `zcv-editor` crate 中实现。

---

## 一、目标

项目只实现一个文本编辑核心：

> 凡是修改普通文本的交互，都使用 `zcv-editor` 中的统一 `Editor`。

文件编辑、Picker 搜索、文件重命名、搜索替换、提交信息等场景，不分别实现输入逻辑，只使用同一个 `Editor` 的不同模式和业务包装。

终端、WebView 等拥有外部输入协议的组件不在此约束内。终端输入需要发送给 PTY，而不是修改 `zcv-engine::Buffer`。

现有 `EditableText`、Pane 手工文本渲染和简单 View 状态均视为原型，不构成新 Editor 的兼容性约束。新 Editor 主干可用后，应一次性替换并删除旧实现，不保留两套编辑核心。

---

## 二、总体架构

Editor 采用与 Zed 相同的核心分层：

```text
zcv-engine::Buffer
  │
  ▼
zcv-language::LanguageBuffer
  ├── Language / SyntaxMap
  ├── 前台语法树插值
  └── single-flight 后台增量解析
  │
  │ 文件 LanguageBuffer 由 BufferStore 按规范化路径复用
  ▼
Editor Entity
  ├── EditorMode
  ├── Selections
  ├── SelectionHistory
  ├── ScrollManager
  ├── IME Composition 会话
  ├── 输入与编辑事务
  ├── Undo / Redo
  └── 焦点与命令状态
  │
  ▼
DisplayMap
  ├── Buffer 坐标 → Display 坐标
  ├── Soft Wrap
  ├── Fold
  ├── Inlay
  └── 可见行计算
  │
  ▼
EditorElement
  ├── 文本布局
  ├── 命中测试
  ├── 光标与选区绘制
  ├── 鼠标交互
  └── GPUI IME / InputHandler
```

各层只能依赖它的下层数据，不允许业务组件绕过 Editor 直接实现文本输入、光标、选区或滚动。

crate 依赖保持单向：

```text
zcv ──▶ zcv-editor ──▶ zcv-workspace
  │          ├───────▶ zcv-engine
  │          ├───────▶ zcv-language ──▶ zcv-engine
  │          └───────▶ zcv-theme
  └──────────────────▶ zcv-language

zcv-picker ──▶ zcv-ui::ErasedEditor ◀── zcv-editor
```

箭头表示“依赖”。`zcv-editor` 只依赖 `zcv-workspace` 中不含 Pane、布局或业务编排的
Item 协议，并直接实现 `Item`；它仍然不依赖具体 Workspace 界面。`EditorEvent` 在
Editor 自身的 Item 实现中转换为通用 `ItemEvent`，不保留第二层 EditorItem 包装。

---

## 三、核心模块边界

### 3.1 Buffer

`zcv-engine::Buffer` 是文本真相来源，负责：

- 存储文本。
- 校验编辑范围。
- 执行插入、删除和替换。
- 生成不可变 Snapshot。
- 管理文本编辑事务、文本历史及 Undo / Redo。

Buffer 不负责：

- GPUI 焦点。
- 光标和选区。
- 屏幕滚动位置。
- 文本布局和绘制。
- 鼠标与键盘事件。
- 当前 Editor 的 SelectionSet。
- SelectionHistory。
- 当前输入焦点的 IME composition 会话。

Buffer 与 Editor 必须分离。同一个 Buffer 可以被多个 Editor 共享；每个 Editor 分别持有自己的选区和滚动状态。当前工作区只装配一个中心 Pane，不预置分栏结构。

文件 Buffer 由 `LanguageBuffer` 持有，宿主层的 BufferStore 创建并索引
`Entity<LanguageBuffer>`。BufferStore 按规范化路径复用仍然存活的 LanguageBuffer，
只保留弱引用，不持有选区、滚动或 Editor 生命周期状态。
BufferStore 由项目级 `Project` Entity 持有，不注册为 App Global；`Workspace` 通过 `Entity<Project>` 打开、保存和监听文件，避免多个窗口或项目共享同一个全局缓存。

Buffer 不得持有 `selection` 字段，也不得通过 `selection()` / `set_selection()` 暴露全局当前选区。接受选区的编辑入口必须把 SelectionSet 作为参数，并把编辑后的 SelectionSet 作为结果返回。

### 3.2 Selection 原语与状态所有权

Selection 分为两类职责：

```text
zcv-engine
  └── Selection 数据原语与纯算法

Editor
  └── 当前视图的 SelectionSet、SelectionHistory 和交互状态
```

`zcv-engine` 保留：

- `Selection` 和 `SelectionSet` 数据类型。
- anchor / head 方向语义。
- 多选区排序、合并和归一化。
- 通过 PositionMap 映射选区。
- grapheme、word、subword、symbol 等纯文本边界算法。
- 选区合法性校验。
- 提供可由 Transaction 使用的文本编辑原语。

Editor 持有：

- 当前 `SelectionSet`（视图层选区端点以引擎 `Anchor` 表达：任何文本变更——本编辑器编辑、共享 Buffer 的其他 Editor 编辑、外部加载——之后统一通过 PositionMap 批量映射端点，选区自动跟随；消费时按当前 Snapshot 解析为字节偏移。引擎 `Selection` / `SelectionSet` 仍是编辑算法与历史快照使用的纯数据原语）。
- primary / newest selection 等当前视图状态。
- 垂直移动的目标列或目标像素位置。
- SelectionHistory。
- 鼠标拖选、列选择等交互状态。
- 依赖 DisplayMap 的移动语义。
- 根据 SelectionSet 生成事务 edits，并计算编辑后的 SelectionSet。
- indent / outdent、删除与多光标替换语义。
- 当前输入焦点的 IME composition 会话。

不得为了迁移把选区实现复制或移动到临时源码目录。Git 历史已经提供回溯能力；临时目录只会制造 engine selection、临时 selection、Editor selection 三套边界。

### 3.3 Editor

`Editor` 是 GPUI Entity，负责跨帧状态和编辑行为：

```rust
pub struct Editor {
    buffer: Entity<Buffer>,
    mode: EditorMode,
    selections: SelectionSet,
    selection_history: SelectionHistory,
    scroll_manager: ScrollManager,
    composition: Option<EditorComposition>,
    focus: FocusHandle,
}
```

以上字段只表达最低限度的结构，不要求一次性实现全部最终字段。

Editor 负责：

- 持有 Buffer 引用。
- 管理光标和选区。
- 管理事务对应的 SelectionHistory。
- 管理滚动状态。
- 把 Action 转换为编辑事务或选择移动。
- 管理当前 Editor 的 IME composition 会话。
- 管理只读、占位文本、输入开关等编辑状态。
- 向 EditorElement 提供渲染快照。
- 发送 BufferEdited、SelectionChanged 等事件。

Editor 不负责：

- 业务外壳和边框。
- Picker 过滤逻辑。
- 文件标签管理。
- 提交、搜索等业务行为。

### 3.4 Language 与 SyntaxMap

`zcv-language` 对齐 Zed 的 language crate，集中维护语言注册、tree-sitter grammar、
highlight、brackets、indents、outline、textobjects、injections query 与增量语法树。
`SyntaxSnapshot` 是统一的版本化语法查询门面；高亮、嵌套语言识别、括号匹配、语法
选择扩展、自动缩进、大纲和文本对象不得各自解析文本或创建第二棵语法树。
`LanguageBuffer` 持有文本 Buffer、SyntaxMap 和唯一的
后台解析任务；多个 Editor 必须共享同一个 LanguageBuffer，不允许各自创建 SyntaxMap。
SyntaxMap 分别记录真正完成解析的 `parsed_version` 和仅通过 `InputEdit` 推进坐标的
`interpolated_version`。渲染端只能消费绑定 `BufferVersion` 的不可变
`SyntaxSnapshot`，并对整个可见字节范围执行一次 capture 查询。
主题解析和 GPUI `TextRun` 不进入语言 crate。

`zcv-engine` 继续保持纯文本内核。LanguageBuffer 独立订阅文本变化并推进 SyntaxMap；
Editor 的独立订阅只推进 DisplayMap。语言状态更新后由 LanguageBuffer 通知所有共享
Editor，避免解析任务和语法树跟随视图重复创建。

### 3.5 DisplayMap

DisplayMap 负责把 Buffer 中的逻辑文本映射为屏幕上的显示文本。

坐标转换方向：

```text
ByteOffset / BufferPoint
          │
          ▼
DisplayPoint
          │
          ▼
像素坐标
```

三种坐标不允许混用：

- `ByteOffset` / `BufferPoint` 表示 Buffer 中的位置。
- `DisplayPoint` 表示经过软换行、折叠和 Inlay 变换后的显示位置。
- 像素坐标只在布局和命中测试阶段使用。

DisplayMap 持有 FoldMap；FoldMap 发布不可变 FoldSnapshot。
FoldSnapshot 同时持有输入
Buffer Snapshot、折叠范围树和输入/输出变换树，保证三者始终属于同一版本。
Fold 与 Transform 直接作为 SumTree item；不存在 FoldSet 或 Projection 状态容器。
FoldMap 通过 `read` 同步 Buffer 变化，通过 `write` 返回 FoldMapWriter 处理折叠操作，
并把 FoldSnapshot 与 FoldEdit 一起交给上层。
TabMap 以上一层 FoldSnapshot 为输入，并把它继续固化在 TabSnapshot 中。
与 Zed 一样，每层 `new` 同时返回 Map 与初始 Snapshot，每层 `sync` 接收下层
Snapshot 并返回本层 Snapshot。FoldSnapshot 与 TabSnapshot 各自维护独立的显示层
版本，因此不修改 Buffer 的折叠操作也能可靠地向上传播。
订阅事件只负责唤醒，当前 `Snapshot` 是文本真相；每层直接理解组合 `TextPatch` 的
旧/新坐标，不缓存或逐条回放中间 Snapshot。
TabMap 按实际投影视口惰性测量 display-column，并在编辑后只失效受影响的已测量行；
初次构建不扫描全文。折叠占位符是 fold 模型内的文本（合并行 = anchor 全文 + 占位符 +
闭合行尾段，闭合括号保留为真实可见文本），像素布局和命中测试由 DisplayMap /
EditorElement 负责。Soft Wrap、Inlay 后续继续在此层扩展，不能散落到 Editor 或业务组件中。

### 3.6 EditorElement

EditorElement 是 Editor 的 GPUI 渲染与输入桥接层，负责：

- 请求和计算布局。
- 测量文字。
- 计算可见行。
- 绘制文本、光标和选区。
- 把鼠标像素位置映射为 DisplayPoint。
- 注册 Editor Actions。
- 建立 GPUI `ElementInputHandler`。
- 提供 IME 候选框所需的光标坐标。

Editor Entity 持有状态，EditorElement 消费状态并完成一帧的布局和绘制。不得把选区、滚动等跨帧状态存入 EditorElement。

---

## 四、EditorMode

所有普通文本输入场景使用同一个 Editor，通过模式控制布局和行为：

```rust
pub(crate) enum EditorMode {
    SingleLine,
    AutoHeight {
        min_lines: usize,
        max_lines: Option<usize>,
    },
    Full,
}
```

### SingleLine

适用于：

- Picker 搜索框。
- 文件重命名。
- 设置输入项。
- 单行搜索条件。

约束：

- 固定为一行高度。
- 不允许插入换行。
- Enter 由外层业务组件解释为确认等 Action。

### AutoHeight

适用于：

- 搜索与替换。
- Git 提交信息。
- 对话或提示词输入。

约束：

- 高度随内容行数增长。
- 不低于 `min_lines`。
- 设置 `max_lines` 后，超过最大高度时在 Editor 内部滚动。

### Full

适用于：

- 文件编辑。
- Diff 编辑。
- Notebook 代码单元格等完整编辑场景。

约束：

- 填满父容器。
- 支持多行、滚动、行号、语法高亮等完整编辑能力。

### 构造入口

对外提供少量明确的构造函数：

```rust
Editor::single_line(...)
Editor::auto_height(...)
Editor::for_buffer(...)
```

业务组件不得通过手工组合内部字段创建不同种类的编辑器。

---

## 五、输入架构

### 5.1 文本输入

文本输入必须使用 GPUI 输入协议：

```text
操作系统文本输入 / IME
          │
          ▼
ElementInputHandler
          │
          ▼
EntityInputHandler for Editor
          │
          ▼
Editor 编辑事务
          │
          ▼
zcv-engine::Buffer
```

禁止通过 `on_key_down` 读取字符并直接写入 Buffer。`on_key_down` 只用于不能表达为文本输入的按键或 Action 路由。

使用 InputHandler 的原因：

- 支持中文、日文等输入法组合文本。
- 支持 emoji 和组合字符。
- 支持 UTF-16 range 与系统输入协议。
- 正确定位输入法候选框。
- 正确处理 marked text 和文本替换范围。

IME composition 的会话身份与 marked ranges 属于当前获得输入焦点的 Editor，不能作为 engine 的全局状态。GPUI 把操作系统的 marked text、相对选区、替换范围和提交事件交给 Editor；Editor 负责 start、update、commit、cancel 生命周期以及候选框定位。多光标场景为每个 selection 保存一个 marked range，系统输入协议只暴露 primary range。

对齐 Zed，marked text 从第一次 preedit update 起就通过普通文本事务进入 Buffer。连续候选更新与第一次输入合并为同一个历史节点，提交只结束 composition 会话，不再次写入文本；取消或空 replacement 也走同一个事务。这样 Buffer Snapshot、SyntaxMap、DisplayMap、命中测试和绘制始终共享同一份文本，组合区域只在语法样式之上额外叠加下划线。

engine 仍不定义 `CompositionState`、`CompositionSelection`、IME 状态机或 `TransactionSource::Composition`。它只提供普通事务和确定性的历史合并；Editor 保存 marked range 与 IME 历史事务身份，并负责选择合并策略。

### 5.2 命令输入

移动、删除、撤销、复制等命令继续遵循项目的 Action 路由：

```text
按键
  │
  ▼
KeyBinding
  │
  ▼
Action
  │
  ▼
Editor handler
```

字符输入和命令输入是两条不同路径：

- 输入文字走 InputHandler。
- 编辑命令走 Action。

不得在 `on_key_down` 中重新实现 Cut、Copy、Paste、Undo、Redo 等命令。

---

## 六、选区与光标

Editor 不单独保存一个裸光标偏移，而是持有 engine 提供的 SelectionSet。Selection 是 engine 中的纯数据原语：

```rust
pub struct Selection {
    anchor: ByteOffset,
    head: ByteOffset,
}
```

- `anchor == head` 表示光标。
- `anchor != head` 表示选区。
- `anchor` 与 `head` 的顺序表达选区方向。

SelectionSet 从一开始按集合设计：

```rust
pub struct SelectionSet {
    selections: Vec<Selection>,
    primary_index: usize,
}
```

第一阶段可以只允许一个 Selection，但底层接口不得绑定为单光标，以免后续加入多光标时重写所有编辑命令。

所有光标移动必须遵守 Unicode 字符或字素边界，禁止直接对 UTF-8 字节偏移执行 `+ 1` 或 `- 1`。

Editor 在 SelectionSet 之外保存 newest、垂直移动 goal、鼠标拖选等视图状态；这些状态可能依赖 DisplayMap，不得反向加入 engine 的 Selection 原语或 Buffer。

---

## 七、编辑事务

所有文本修改必须转换成统一编辑事务：

```text
键盘输入
粘贴
删除
自动缩进
接受补全
格式化
    │
    ▼
Editor Transaction
    │
    ▼
一组 TextEdit
    │
    ▼
Editor 提交 Buffer Transaction
    │
    ▼
EditOutcome + after_selections
```

事务至少需要保证：

- 一次用户操作可以整体 Undo。
- 多个 Selection 的编辑保持原子性。
- 编辑后 Selection 能映射到新位置。
- BufferEdited 事件只在事务提交后发送。
- 业务组件不直接修改 Buffer 并自行修正光标。

Editor 的选区编辑层使用以下形态：

```rust
pub struct EditOutcome {
    transaction: Option<TransactionOutcome>,
    after_selections: SelectionSet,
}

editor.replace_selections(
    &mut buffer,
    &editor.selections,
    text,
    metadata,
) -> EngineResult<EditOutcome>;
```

`transaction = None` 表示文本未变化，但 `after_selections` 仍然有效。实际提交发生时，`TransactionOutcome` 提供本次提交的 `transaction_id`、可选的 `history_transaction_id`、Delta 和 ChangeSet。

Buffer 只保存文本历史。Editor 在事务开始和结束时，以 `history_transaction_id` 为键记录自己的前后选区：

```text
history_transaction_id
  ├── undo selections
  └── redo selections
```

Undo / Redo 由 Buffer 回放文本并返回同一个规范 `transaction_id`；Editor 再从自己的 SelectionHistory 恢复对应选区。历史合并发生时，多次实际提交共享首个历史节点的 `history_transaction_id`，Editor 应更新该键对应的 redo selections。Buffer 的 HistoryEntry 和 TransactionRecord 不得保存某个 Editor 的 `before_selection` / `after_selection`。

---

## 八、滚动与可见区域

滚动状态属于 Editor，而不是 Buffer 或 Pane：

```rust
pub(crate) struct ScrollManager {
    anchor: DisplayPoint,
    offset: Point<Pixels>,
}
```

职责包括：

- 保存垂直和水平滚动位置。
- 根据光标移动执行自动滚动。
- 计算当前可见显示行范围。
- 响应滚轮、PageUp、PageDown 等命令。

Pane 只能提供 Editor 的可用尺寸，不得读取全文、手工切行或维护自己的编辑器滚动行号。

---

## 九、业务组件接入规则

### Picker

Picker 通过 `zcv-ui` 的类型擦除门面持有由 `zcv-editor` 工厂创建的
`Editor::single_line`。该门面不实现文本编辑，只用于打破 Picker 与 Workspace 的 crate 依赖环。
Picker 订阅 Editor 的 Edited 事件并更新匹配项。

Picker 不实现：

- 字符输入。
- 光标移动。
- 剪贴板。
- 占位文本绘制。

### Pane

Pane 通过对象安全的 ItemHandle 持有异构 Item，当前文件标签对应一个完整 Editor
Entity：

```text
Pane
  └── Active EntityId
        └── Box<dyn ItemHandle>
              └── Entity<Editor>
```

具体视图实现强类型 `Item`，`Entity<T: Item>` 通过统一桥接实现 `ItemHandle`。
Pane 负责标签切换和容器布局，Editor 负责内容编辑与绘制。业务确实需要 Editor
能力时，通过统一的 `active_item_as::<Editor>()` 受控向下转型。

多个 Editor 可以共享同一个 Buffer，但不能共享 Selection 和 ScrollManager。

### 文件视图生命周期

Pane 直接持有 `Box<dyn ItemHandle>`，以 EntityId 作为唯一 Item 身份，并通过 Item
暴露的文件路径负责当前 Pane 内的文件去重与 Editor 生命周期。`ViewRegistry` 和独立
`ViewId` 已删除，不再维护重复的身份、路径、Buffer 或 Editor 映射。

跨 Pane 打开同一文件时，由 BufferStore 按规范化路径复用 `Entity<Buffer>`，各 Pane
分别创建并持有独立 Editor，从而共享文本内容但保持 Selection 和 ScrollManager 独立。

### 其他输入组件

文件重命名、设置输入框、搜索替换等业务组件只负责：

- 选择 EditorMode。
- 设置占位文本和只读状态。
- 提供视觉外壳。
- 订阅 Editor 事件。
- 处理确认、取消等业务 Action。

不得创建新的 `EditableText`、`TextInput` 或其他独立文本编辑实现。

---

## 十、开发阶段

开发按主干能力推进，不按业务页面分别实现输入框。

### 阶段 0：拆分 Buffer 与 Selection 状态

本阶段是 Editor 开发的前置里程碑，必须在 Editor 状态、输入和 Undo / Redo 建立之前完成。它不是独立的大型重构，也不得推迟到 Editor 开发中途。

状态：已完成。

- 保留 engine 的 Selection、SelectionSet 和纯文本算法。
- 把 selection 编辑入口迁入 Editor，输入 SelectionSet、返回 Editor 私有的 EditOutcome。
- 从 Buffer 删除当前 `selection` 字段。
- 删除 `Buffer::selection()` 和 `Buffer::set_selection()`。
- 删除普通事务对 Buffer 当前选区的隐式映射。
- 从 Buffer HistoryEntry 和 TransactionRecord 删除前后选区。
- 让 Buffer Undo / Redo 返回 `transaction_id`。
- 从 engine 删除 IME composition 类型、状态机和专用事务来源。
- 保证 engine 不持有 IME 会话状态；marked text 由 Editor 作为普通可合并事务写入 Buffer。

验证：

- Buffer 不保存任何当前视图选区。
- 多个 Editor 可以基于同一个 Buffer 分别生成独立 selection 事务。
- Editor selection 编辑返回正确的 after_selections。
- Undo / Redo 能返回供 Editor 恢复选区的 transaction_id。
- engine 的选区归一化、Unicode 边界和映射测试继续通过。

### 阶段 1：Editor 状态骨架

- 定义 `Editor`。
- 定义 `EditorMode`。
- 使用 engine 的 `Selection` 和 `SelectionSet`。
- 定义 Editor 自己的 `SelectionHistory`。
- 接入 `zcv-engine::Buffer`。
- 提供 `single_line`、`auto_height`、`for_buffer` 构造函数。

验证：

- 两个 Editor 能共享一个 Buffer。
- 两个 Editor 的选区和滚动状态相互独立。

### 阶段 2：DisplayMap 基础映射

- 定义 BufferPoint 和 DisplayPoint。
- 实现无折叠、无软换行时的双向映射。
- 通过 Snapshot 读取文本。

验证：

- 任意合法 BufferPoint 都能映射到 DisplayPoint 并还原。
- 多字节 Unicode 文本的映射正确。

### 阶段 3：EditorElement

- 实现文本布局。
- 绘制单行和多行文本。
- 绘制光标与选区。
- 实现像素位置命中测试。

验证：

- 光标绘制位置与文字测量结果一致。
- 鼠标点击可以得到正确的 BufferPoint。

### 阶段 4：IME 与文本输入

- 为 Editor 实现 `EntityInputHandler`。
- 在 EditorElement 中注册 `ElementInputHandler`。
- 在 Editor 中定义并持有 `EditorComposition` 会话状态。
- 使用 GPUI 输入协议维护 marked text、相对选区和替换范围。
- 从第一次 preedit update 起用普通 Selection 编辑事务更新 Buffer。
- 将连续候选更新合并为同一个 Undo / Redo 历史节点。
- 在正常语法高亮之上为所有 marked ranges 叠加下划线。
- 收到 committed text 或 unmark 后结束会话，不重复写入已在 Buffer 中的文本。
- 实现 committed text、marked text 和替换范围。
- 提供 IME 候选框坐标。

验证：

- 中文输入法可以输入、组合、确认和取消。
- emoji、组合字符和多字节字符不会破坏 Buffer。
- 组合期间 Buffer、SyntaxMap、DisplayMap 与绘制文本版本一致，语法高亮不中断。
- 一次完整的候选更新序列只产生一个 Undo 步骤。
- 多光标的每次候选更新覆盖所有 marked ranges，primary range 与系统输入协议一致。

### 阶段 5：选择与编辑命令

- 实现字符、单词、行级移动。
- 实现扩展选区。
- 实现插入、删除、换行。
- 实现 Cut、Copy、Paste。
- 接入 Undo / Redo。

验证：

- 所有文本修改都经过事务。
- Undo / Redo 同时恢复文本和 Selection。

### 阶段 6：滚动和可见行

- 实现 ScrollManager。
- 只布局和绘制可见行。
- 实现自动滚动和滚轮输入。

验证：

- 大文件不会为全文创建 GPUI 子元素。
- 光标移动到视口外时自动滚动。

### 阶段 7：接管现有场景

- Pane 改用 `Editor::for_buffer`。
- Picker 改用 `Editor::single_line`。
- 删除 Pane 手工文本渲染。
- 删除旧 `EditableText`。
- 删除重复的滚动和输入状态。

验证：

- 项目中普通文本编辑只存在一个实现。
- 搜索 `EditableText`、手工字符输入和 Pane 手工切行时无残留。

### 阶段 8：高级显示能力

按真实需求依次增加：

- Soft Wrap。
- 语法高亮。
- Fold。
- Inlay。
- 多光标。
- 补全和诊断。

每项能力都必须放入既定层次，不得绕过 DisplayMap 或 Editor 事务系统。

---

## 十一、文件组织

当前目录：

```text
zcv-editor/
  Cargo.toml
  src/
    lib.rs
    view/
      mod.rs
      test/
    element.rs
    display_map.rs
    display_map/
      error.rs
      fold_map.rs
      tab_map.rs
    selection.rs
    scroll.rs
```

规则：

- `view/mod.rs` 定义 Editor、EditorMode、事件和 Action handler，相关交互测试放在
  `view/test/`。
- `element.rs` 负责布局、绘制、命中测试和 InputHandler 桥接。
- `display_map.rs` 负责编排显示变换层；具体能力放在 `display_map/` 下对应的 Map 模块中。
- `selection.rs` 负责 Editor 的 SelectionHistory、视图交互状态和依赖 DisplayMap 的选择变换；Selection、SelectionSet 原语继续来自 engine。
- `scroll.rs` 负责 ScrollManager。
- `lib.rs` 作为 Editor crate 门面，声明私有实现模块，并选择性重导出宿主使用的
  主类型、事件和 Action；具体规则见《重导出规范》。
- `zcv` 直接从 `zcv_editor` 导入，不建立 `crate::editor` 兼容门面。

文件只在实现确实形成独立职责时建立，不预先复制 Zed 的全部目录结构。

---

## 十二、禁止事项

- 禁止新增第二套文本输入组件。
- 禁止用 `on_key_down` 处理普通字符输入。
- 禁止按 UTF-8 字节递增或递减光标。
- 禁止 Pane 或 Picker 直接修改 Buffer。
- 禁止业务组件自行实现 Selection、Undo、剪贴板或 IME。
- 禁止 Buffer 持有某个 Editor 的当前 SelectionSet。
- 禁止 Buffer 持有当前输入焦点的 IME composition 会话。
- 禁止 Buffer 文本历史绑定某一个 Editor 的前后选区。
- 禁止在 Pane 和 Editor 中重复保存滚动状态。
- 禁止为短期演示绕过编辑事务直接改文本。
- 禁止在没有真实需求时照搬 Zed 的 LSP、协作、补全等大型子系统。

---

## 十三、完成标准

统一 Editor 主干完成时，应满足：

- 项目中只有一个普通文本编辑核心。
- SingleLine、AutoHeight、Full 共用相同的 Editor 和 EditorElement。
- 文件编辑与 Picker 输入使用同一套输入、选区和事务逻辑。
- 支持完整 IME 输入和 Unicode 光标移动。
- Buffer、Editor、DisplayMap、EditorElement 的职责没有交叉。
- Buffer 文本历史与 Editor SelectionHistory 已按 transaction_id 解耦。
- 同一 Buffer 可以被多个独立 Editor View 共享。
- Pane 和业务组件不再手工渲染或修改编辑文本。
