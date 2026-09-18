# zcv-preview-svg

`zcv-preview-svg` 把 SVG 源码光栅化为图像预览（源码派生 `Source`、画布 `Canvas`）。

公共入口是 [`src/preview_svg.rs`](src/preview_svg.rs)。

## 职责

- 从源码 `MultiBuffer` 读取文本，在后台按内容缩放光栅化 SVG。
- 识别 SVG 扩展并注册 `PreviewProvider`。
- 提供源码与预览切换工具栏。

## 所有权

- 视图持有光栅化状态、任务句柄和 `PreviewToolbar`。
- 源码文本由源码 `MultiBuffer` 拥有，视图只读取只读快照。

## 不变量

- 画布预览必须经 `zcv_workspace::PreviewViewport` 展示。
- 工具栏复用 `zcv_workspace::PreviewToolbar`。
- 光栅化在后台执行，渲染线程不阻塞。

## 验证

```bash
cargo check -p zcv-preview-svg
cargo test -p zcv-preview-svg
```
