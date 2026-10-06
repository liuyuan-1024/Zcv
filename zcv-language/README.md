# zcv-language

`zcv-language` 负责文件语言识别、Tree-sitter 解析、高亮、语言注入和结构查询。编辑器只消费 `LanguageBuffer` 与 `SyntaxSnapshot`，不单独维护语言状态。

`LanguageBuffer` 直接持有文本 `Buffer` 与语法状态；`LanguageBuffer::snapshot()` 是文本与语法的一致读取边界：返回前先把语法插值到文本版本，并同时给出按语言解析的 `LanguageSettings` 与派生高亮缓存句柄；消费方不再调用任何手动同步协议，也不通过第二个实体读取文本。同源的 `text_snapshot()` 只读取同一份权威文本，不构成第二数据源。语言注册表 `LanguageRegistry` 由应用装配层创建并以 `Arc` 显式注入，`zcv-language` 不提供全局单例。

文本编辑后先插值语法树坐标；没有在途解析时，在编辑线程内给增量解析约 1 ms 的预算。预算耗尽才转入后台；在途期间的新编辑合并为任务完成后的一次补解析。前台与后台结果共用安装入口，只有与当前文本版本和语言匹配的结果才能安装。插值快照的文本版本已更新，但节点类别可能要等解析完成后才更新；解析安装通过 `Reparsed` 事件推进上层快照。

输入配对、`not_in` 与 `autoclose_before` 由语言规格持有。`overrides.scm` 在装配时编译；其捕获与 `not_in` 名称同时校验。`SyntaxSnapshot::input_scope_at` 在同版本文本上按深度和区间终点索引定位可能覆盖光标的注入层，再选择最深语言层与最窄捕获；结果带语言、作用域、输入政策、文本版本与真正解析版本。没有语法树时使用已知源语言的默认政策；待解析注入层仍保留在语法模型中。

文件级符号使用各语言自己的 `queries/<language>/outline.scm`。`SyntaxSnapshot::outline` 在同一份源快照中使用共享高亮缓存生成标签文本、标签相对高亮和源 `Anchor` 范围，并记录语法层与父子层级；没有该查询的语言明确返回空结果。`MultiBuffer` 把完整落在 excerpt 内的结果映射为组合 `Anchor`，`Editor` 在导航时按当前快照解析锚点。面板刷新期间可以继续绘制旧标签，无需用旧字节范围查询当前文本；捕获名称按当前主题解析样式。

节点导航使用 `SyntaxSnapshot::node_at` 和 `SyntaxSnapshot::node_ancestors`，返回带版本、UTF-8 字节范围、节点种类和语法层的不可变节点摘要。
注入层优先于宿主层，祖先链不跨层；空白没有独立 Tree-sitter 节点时归属于语法根，文件末尾光标按前一个字节查询。
结构化选择通过 `SyntaxSnapshot::expand_selection_range` 逐级取同一语法层的严格祖先，`Editor` 只持有最终选区。
局部绑定使用 `queries/<language>/locals.scm` 的 `@local.scope`、`@local.definition` 和 `@local.reference` capture；
`SyntaxSnapshot::local_bindings` 只返回按作用域确定归属的定义与引用，并携带当前 `BufferVersion`。
当前已为 Rust、Python、JavaScript/JSX、TypeScript/TSX、Go、C 和 C++ 提供首批局部查询；未声明 `locals.scm` 的语言明确不产生局部绑定结果。

## 语言规格

高亮缓存由语言源快照共享，是按文本／语法版本失效的派生数据。高亮按 50 行组成一个缓存块，以行起点作为块边界，保证输出范围落在 UTF-8 字符边界；超过 64 KiB 的块只查询所需范围，不写入缓存。缓存采用 LRU，命中和提升顺序为常数时间；10 MiB 字节预算计入 span 和估算的条目、链表开销，空结果也占用预算，替换与淘汰同步扣减。该预算约束缓存成本，不代表整个进程的实际内存占用。

每门内置语言在 `src/available_languages.rs` 中只有一个 `LanguageSpec`。支持类型只能是：

- `PlainText`：真正的纯文本兜底，不创建语法树；
- `TreeSitter`：必须同时提供 grammar 与高亮查询；可直接识别文件的语言还必须提供括号、缩进和折叠查询。

规格可通过 `with_word_characters` 声明语言默认的额外词字符，例如 JavaScript/TypeScript 的 `$`、`#`。`with_word_character_overrides` 为 `overrides.scm` 的已验证捕获声明完整替换值；当前 JavaScript/JSX 的字符串使用 `.`，注释使用 `-`。这是阶段 3 语境差异验收指定的 Zcv 输入语义；本机 Zed 对应配置目前没有词字符覆盖值，不能将这两个值称为与 Zed 逐项一致。`SyntaxSnapshot::word_scope_at` 在当前文本／语法快照和源位置按注入层与捕获选择政策，文尾按词操作回看前一个字形；普通输入继续用 `input_scope_at` 的文尾边界语义。作用域结果由 `InputScope::word_boundary()` 提供，不保存按源语言复制的长期策略。没有位置的全局搜索读取宿主 `SyntaxSnapshot::global_word_boundary()`。

不要登记只有文件名、没有 grammar 的占位语言。尚未完整支持的文件统一按纯文本打开。

grammar crate 已公开且与 grammar 同版本发布的基础高亮查询，可以直接由语言规格引用；其余语法能力由每门语言在 `queries/<language>/` 下独立提供：

```text
highlights.scm          必需，语法高亮
injections.scm          存在真实嵌套语义时提供
brackets.scm            文件语言必需，括号感知
indents.scm             文件语言必需，换行缩进
folds.scm               文件语言必需，代码折叠
outline.scm             可选，文件级符号与代码大纲
locals.scm              可选，局部绑定与引用
overrides.scm           按语法范围覆盖输入政策时提供
```

### 换行输入政策

`LanguageInputConfig` 是语言层的唯一注释、文档注释和列表续行配置来源。`InputScope` 按当前位置返回这些政策；注入语言和 `overrides.scm` 捕获可以覆盖宿主语言。换行缩进仍只由同版本 `indents.scm` 的 `NewlineIndent` 提供。

JSX／TSX 的 `element` 块注释配置也由语言规格提供。只有语言明确提供文档注释续行配置时才续写块内前缀；例如 CSS 只有块注释配置，Enter 保持普通换行。尚未逐语言核对的输入政策见[语言覆盖表](../docs/语境感知输入阶段0覆盖与样例.md)。

`brackets.scm` 的 `#set! newline.only` 是语法结构规则，与自动闭合配对配置分开。`BracketPair` 保留该属性，组合层只在同一可编辑 excerpt 内、选区处于开闭括号之间且两侧只有非换行空白时返回它；编辑器据此决定额外空行。

结构查询不能跨语言目录共享。即使两门语言当前规则相同，也应分别保存查询文件，让后续语法差异在各自语言边界内演进。语言注入只在存在明确嵌套语义时接入；仅供 Markdown 内部使用的 `Markdown Inline` 不受文件语言的结构查询基线约束。

## 新增语言

1. 在工作区和 `zcv-language` 中加入与当前 Tree-sitter 版本兼容的 grammar 依赖。
2. 引用 grammar crate 自带的高亮查询；crate 未提供或 Zcv 需要定制时，建立 `queries/<language>/highlights.scm`。
3. 在语言目录中提供 `brackets.scm`、`indents.scm` 和 `folds.scm`；存在嵌套语言时再提供 `injections.scm`。
4. 在 `builtin_languages` 中通过 `LanguageSpec::tree_sitter` 登记识别规则、grammar、查询、输入配对与 `autoclose_before`；配对使用 `not_in` 时提供对应 `overrides.scm` 捕获。
5. 高亮 capture 优先复用主题已有名称；新增根 capture 时同步更新深色、浅色主题。
6. 增加文件识别、代表性 capture 和新增结构能力的行为测试。

注册表测试会加载全部内置规格，检查名称和后缀唯一性、规格可达性，并确保每门文件语言同时具有 grammar、高亮、括号、缩进和折叠查询。
