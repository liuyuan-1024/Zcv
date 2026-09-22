# zcv-project

项目数据层：仓库发现、git 状态编排、文件 Buffer 生命周期与项目级搜索。

`Project` 管理项目根、目录快照（Worktree）、按路径复用的文档实体（`BufferStore`）与文件系统监听；`GitStore` 管理仓库与修订文本。窗口布局、Pane、Dock 与其他界面状态由 `zcv-workspace` 管理。

## 文档实体与所有权

- 文件到文档的索引保存弱引用；只要还有 `Editor` 或视图持有文档，就复用同一条文档链，最后一个使用者释放后整条链结束。
- 同一路径只有一个权威文档实体；打开、保存、重载与搜索都经同一条解码与建 Buffer 路径（`buffer_store` + `text_file`），不为同一文件物化第二份可写文档。
- Git 修订文本（HEAD / index）由 `GitStore` 按 `(revision, path)` 唯一持有。
- 文件解码固定为「剥离 UTF-8 BOM、拒绝非法 UTF-8」；保存时换行统一为 LF。

## 边界

- 不依赖 `zcv-multi-buffer` 或 `zcv-editor`；文件边界只面向文档实体与文本快照，不依赖组合文档。
- 不把文本、语法或组合文档的可写权威上移为产品层状态。
- 不引入协作、远程与 LSP。

## 验证

```bash
cargo test -p zcv-project
```
