# zcv-preview-image

`zcv-preview-image` 为栅格图片文件提供独立预览（无源码 Item 的 `Standalone`、`Canvas` 展示）。

公共入口是 [`src/preview_image.rs`](src/preview_image.rs)。

## 职责

- 识别图片扩展并注册 `PreviewProvider`。
- 在画布预览中按工作区内容缩放显示图片。

## 所有权

- Provider 只做格式识别与视图创建，不持有状态。
- 预览视图持有自己的 `PreviewViewport`（滚动、居中、缩放）。

## 不变量

- 画布预览必须经 `zcv_workspace::PreviewViewport` 展示，格式实现不自行读取排版状态。
- 本 crate 不依赖项目领域类型；预览只消费文件路径与宿主能力。

## 验证

```bash
cargo check -p zcv-preview-image
cargo test -p zcv-preview-image
```
