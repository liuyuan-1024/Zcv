# zcv-preview-image

`zcv-preview-image` 为栅格图片文件提供独立预览（无源码 Item 的 `Standalone`、`Canvas` 展示）。图片内容由 `Project` 拥有的 `ImageDocument` 在后台读取与解码，视图只消费文档句柄。

公共入口是 [`src/preview_image.rs`](src/preview_image.rs)。

## 职责

- 识别图片扩展并注册 `PreviewProvider`。
- 在画布预览中按工作区内容缩放显示图片。

## 所有权

- Provider 只做格式识别与视图创建，不持有状态。
- 图片文档由 `Project` 的图片存储按路径拥有并复用；预览视图订阅 `ImageDocument` 的重载事件，不自行读盘或缓存字节。
- 预览视图持有自己的 `PreviewViewport`（滚动、居中、缩放）。
- 预览视图通过 `PreviewItem` 向 Pane 工具区暴露预览身份，并提供文件面包屑；独立图片没有源码 Item。

## 不变量

- 画布预览必须经 `zcv_workspace::PreviewViewport` 展示，格式实现不自行读取排版状态。
- 视图不自行读取或缓存图片字节；内容加载、磁盘状态与重载由 `Project` 的 `ImageDocument` 承担。

## 验证

```bash
cargo check -p zcv-preview-image
cargo test -p zcv-preview-image
```
