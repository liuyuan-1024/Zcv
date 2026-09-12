# zcv-language

`zcv-language` 负责文件语言识别、Tree-sitter 解析、高亮、语言注入和结构查询。编辑器只消费 `LanguageBuffer` 与 `SyntaxSnapshot`，不单独维护语言状态。

文件级符号使用各语言自己的 `queries/<language>/outline.scm`。查询结果由 `SyntaxSnapshot::outline` 产生，携带文本版本、源文件字节范围、名称范围、语法层和父子层级；没有该查询的语言明确返回空结果。`MultiBuffer` 只负责把完整落在 excerpt 内的结果映射到组合文档，`Editor` 提供当前大纲、名称过滤和名称定位入口。

节点导航使用 `SyntaxSnapshot::node_at` 和 `SyntaxSnapshot::node_ancestors`，返回带版本、UTF-8 字节范围、节点种类和语法层的不可变节点摘要。
注入层优先于宿主层，祖先链不跨层；空白没有独立 Tree-sitter 节点时归属于语法根，文件末尾光标按前一个字节查询。
结构化选择通过 `SyntaxSnapshot::expand_selection_range` 逐级取同一语法层的严格祖先，`Editor` 只持有最终选区。
局部绑定使用 `queries/<language>/locals.scm` 的 `@local.scope`、`@local.definition` 和 `@local.reference` capture；
`SyntaxSnapshot::local_bindings` 只返回按作用域确定归属的定义与引用，并携带当前 `BufferVersion`。

## 语言规格

每门内置语言在 `src/available_languages.rs` 中只有一个 `LanguageSpec`。支持类型只能是：

- `PlainText`：真正的纯文本兜底，不创建语法树；
- `TreeSitter`：必须同时提供 grammar 与高亮查询；可直接识别文件的语言还必须提供括号、缩进和折叠查询。

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
```

结构查询不能跨语言目录共享。即使两门语言当前规则相同，也应分别保存查询文件，让后续语法差异在各自语言边界内演进。语言注入只在存在明确嵌套语义时接入；仅供 Markdown 内部使用的 `Markdown Inline` 不受文件语言的结构查询基线约束。

## 新增语言

1. 在工作区和 `zcv-language` 中加入与当前 Tree-sitter 版本兼容的 grammar 依赖。
2. 引用 grammar crate 自带的高亮查询；crate 未提供或 Zcv 需要定制时，建立 `queries/<language>/highlights.scm`。
3. 在语言目录中提供 `brackets.scm`、`indents.scm` 和 `folds.scm`；存在嵌套语言时再提供 `injections.scm`。
4. 在 `builtin_languages` 中通过 `LanguageSpec::tree_sitter` 登记识别规则、grammar、查询和输入配对。
5. 高亮 capture 优先复用主题已有名称；新增根 capture 时同步更新深色、浅色主题。
6. 增加文件识别、代表性 capture 和新增结构能力的行为测试。

注册表测试会加载全部内置规格，检查名称和后缀唯一性、规格可达性，并确保每门文件语言同时具有 grammar、高亮、括号、缩进和折叠查询。
