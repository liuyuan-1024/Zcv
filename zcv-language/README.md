# zcv-language

`zcv-language` 负责文件语言识别、Tree-sitter 解析、高亮、语言注入和结构查询。编辑器只消费 `LanguageBuffer` 与 `SyntaxSnapshot`，不单独维护语言状态。

## 语言规格

每门内置语言在 `src/available_languages.rs` 中只有一个 `LanguageSpec`。支持类型只能是：

- `PlainText`：真正的纯文本兜底，不创建语法树；
- `TreeSitter`：必须同时提供 grammar 与 `highlights.scm`，可按需提供注入、括号、缩进和折叠查询。

不要登记只有文件名、没有 grammar 的占位语言。尚未完整支持的文件统一按纯文本打开。

grammar crate 已公开且与 grammar 同版本发布的基础高亮查询，可以直接由语言规格引用；Zcv 自定义或补充的查询放在 `queries/<language>/` 下：

```text
highlights.scm          必需，语法高亮
injections.scm          可选，嵌套语言
brackets.scm            可选，括号感知
indents.scm             可选，换行缩进
fold.scm / folds.scm    可选，代码折叠
```

当前可直接通过文件识别的 Tree-sitter 语言都提供高亮、括号、缩进和折叠查询。语言注入只在存在明确嵌套语义且目标语言已注册时接入；仅供 Markdown 内部使用的 `Markdown Inline` 不受这一文件语言基线约束。

## 新增语言

1. 在工作区和 `zcv-language` 中加入与当前 Tree-sitter 版本兼容的 grammar 依赖。
2. 引用 grammar crate 自带的高亮查询；crate 未提供或 Zcv 需要定制时，建立 `queries/<language>/highlights.scm`。需要其他语法能力时再添加对应查询文件。
3. 在 `builtin_languages` 中通过 `LanguageSpec::tree_sitter` 登记识别规则、grammar、查询和输入配对。
4. 高亮 capture 优先复用主题已有名称；新增根 capture 时同步更新深色、浅色主题。
5. 增加文件识别、代表性 capture 和新增结构能力的行为测试。

注册表测试会加载全部内置规格，检查名称和后缀唯一性、规格可达性，并确保每门 Tree-sitter 语言同时具有 grammar 与高亮查询。
