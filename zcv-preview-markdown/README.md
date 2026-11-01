# zcv-preview-markdown

`zcv-preview-markdown` 把 Markdown 源码投影为原生块元素预览（源码派生 `Source`、流式排版 `Flow`）。

公共入口是 [`src/preview_markdown.rs`](src/preview_markdown.rs)。

## 职责

- 解析源码 `MultiBuffer` 的 Markdown 文本并渲染为块/行内元素。
- 对代码围栏做语法高亮、对公式和 Mermaid 图表做后台渲染。
- 为每个块记录源码来源区间，为标题生成文档内锚点；点击 `#标题` 链接、从大纲跳转或源码/预览切换时，把 `MultiBufferAnchor` 解析为预览滚动位置。预览位置以锚点保存，不使用裸像素偏移。
- Markdown 预览支持拖拽选择、全选和复制；复制结果是渲染后的纯文本，不包含 Markdown 标记。
- 渲染预览内容；源码与预览切换由工作区的工具项承担。

## 所有权

- 视图持有解析后的块、代码高亮任务、公式与 Mermaid 图片缓存，不持有工具栏视图。
- 视图持有预览专属的选择、滚动和命中测试状态；这些状态不修改源码文档，也不进入其他预览类型。
- 阅读位置以 `MultiBufferAnchor` 保存；源码/预览标签切换只做一次锚点交接，不建立持续双向像素同步。
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
