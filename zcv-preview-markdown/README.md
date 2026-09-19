# zcv-preview-markdown

`zcv-preview-markdown` 把 Markdown 源码投影为原生块元素预览（源码派生 `Source`、流式排版 `Flow`）。

公共入口是 [`src/preview_markdown.rs`](src/preview_markdown.rs)。

## 职责

- 解析源码 `MultiBuffer` 的 Markdown 文本并渲染为块/行内元素。
- 对代码围栏做语法高亮、对公式做后台渲染。
- 渲染预览内容；源码与预览切换由工作区的工具项承担。

## 所有权

- 视图持有解析后的块、代码高亮任务与公式图片缓存，不持有工具栏视图。
- 源码文档内容由源码 `MultiBuffer` 拥有，视图只读取快照。
- 代码围栏高亮使用的语言注册表来自源码 `MultiBuffer`。

## 不变量

- 源码 `MultiBuffer` 必须携带语言注册表；缺失即装配顺序错误，显式失败，不静默退化为无高亮。
- 源码与预览切换由 `zcv_workspace::PreviewToolbar` 作为 Pane 工具项统一承担，预览视图不持有也不返回工具栏视图。
- 渲染始终从窗口/工作区读取内容排版。

## 验证

```bash
cargo check -p zcv-preview-markdown
cargo test -p zcv-preview-markdown
```
