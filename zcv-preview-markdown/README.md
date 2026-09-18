# zcv-preview-markdown

`zcv-preview-markdown` 把 Markdown 源码投影为原生块元素预览（源码派生 `Source`、流式排版 `Flow`）。

公共入口是 [`src/preview_markdown.rs`](src/preview_markdown.rs)。

## 职责

- 解析源码 `MultiBuffer` 的 Markdown 文本并渲染为块/行内元素。
- 对代码围栏做语法高亮、对公式做后台渲染。
- 提供源码与预览切换工具栏。

## 所有权

- 视图持有解析后的块、代码高亮任务、公式图片缓存和 `PreviewToolbar`。
- 源码文档内容由源码 `MultiBuffer` 拥有，视图只读取快照。
- 代码围栏高亮使用的语言注册表来自源码 `MultiBuffer`。

## 不变量

- 源码 `MultiBuffer` 必须携带语言注册表；缺失即装配顺序错误，显式失败，不静默退化为无高亮。
- 工具栏复用 `zcv_workspace::PreviewToolbar`，不复制面包屑/返回源码实现。
- 渲染始终从窗口/工作区读取内容排版。

## 验证

```bash
cargo check -p zcv-preview-markdown
cargo test -p zcv-preview-markdown
```
