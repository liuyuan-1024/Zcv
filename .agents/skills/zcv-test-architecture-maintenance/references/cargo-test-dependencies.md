# Cargo 测试依赖审计

## 审计目标

对每个依赖确认它服务于哪类目标：正常库或二进制、测试、benchmark、example、fuzz，还是构建脚本。只服务测试目标的 crate 应放在 `[dev-dependencies]`；不要因为测试模块位于生产源文件，就把测试 crate 留在 `[dependencies]`。

同时检查 workspace 继承、features 和目标平台条件，避免测试 feature 通过共享依赖配置进入正常生产构建。

## 推荐证据

根据修改范围选择最小检查：

- 阅读目标 crate 的 `Cargo.toml`，确认依赖分区和 feature 来源；
- 用 `cargo tree` 区分正常依赖与开发依赖，检查测试 crate 是否仍被正常目标引入；
- 用目标 package 的正常 `cargo check` 或等价类型检查确认生产构建不再依赖测试支持；
- 用明确的测试目标命令确认迁移后的 fixture 和依赖仍可用。

不要仅凭 `cargo test` 通过就断言生产依赖边界正确；测试构建可能额外启用了开发依赖和 features。

## 修改原则

- 移动依赖时同步检查其 feature、workspace 继承和所有调用方。
- 不要为了消除一处编译错误，把测试 crate 留回生产依赖。
- 如果测试支持代码必须由多个 crate 共享，优先用独立 crate 表达依赖边界；只有现有架构明确适合时才增加测试专用 feature。
- 不要添加没有当前消费者的抽象、feature 或兼容层。
