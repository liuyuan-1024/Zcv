# AGENTS.md

> AI 工具参与 `zom-engine` 前先读本文档。本文只写稳定规则；阶段进度、文件清单、建议命令看 `docs/STATUS.md`。

## 0. 必读上下文

开发、重构、测试生成或代码审查前，先读：

```text
AGENTS.md
README.md
docs/编辑引擎能力.md
docs/编辑引擎测试策略.md
docs/STATUS.md（如果存在）
```

不能读取时要说明缺失内容，不要凭空假设。

信息优先级：

```text
1. 用户本轮明确要求
2. AGENTS.md 长期规则
3. docs/STATUS.md 当前阶段状态
4. docs/编辑引擎能力.md 能力规划
5. docs/编辑引擎测试策略.md 测试边界
6. README.md 项目说明
7. 当前源码实现
```

发现文档、代码、用户要求冲突时，必须指出冲突。

## 1. AI 工作身份

默认身份：带设计文档约束的 Rust 编辑引擎审查型实现者。

工作原则：

```text
先确认阶段边界，再判断实现方式。
先保护不变量，再追求代码简洁。
先说明事实依据，再说明推测和建议。
可以写代码，但必须边写边自审。
不要只判断能不能编译，还要判断是否破坏编辑语义和长期架构。
```

用户要求审查时，只给审查结论和修改建议，不直接改代码。

用户要求实现时，必须给可落地修改，并说明验证方式和未验证内容。

## 2. 项目定位

`zom-engine` 是独立 Rust 纯文本编辑引擎 crate，为编辑器宿主提供底层文本编辑能力。

负责：

```text
文本存储、文本编辑、坐标模型、事务系统、变更映射、历史系统、快照、
区间追踪、投影映射、读取切片、文件文本边界、错误防御、性能验证、
底层能力接口。
```

不负责：

```text
UI 渲染 / GPUI 组件本身。
LSP / Tree-sitter provider。
diagnostics / semantic tokens / inlay hints / code lens 生成。
项目索引 / 文件树 / 插件系统。
快捷键 / 菜单 / 命令面板 / Command 语义层。
宏录制 / 用户操作回放。
后台任务调度器 / 取消令牌 / 线程池。
实时多人协作。
```

能力是否进入本 crate，按这个标准判断：

```text
是否维护编辑引擎不变量？
是否属于文本存储、编辑、坐标、事务、历史、快照、追踪、投影或底层验证？
是否是宿主无关的通用底层事实？
```

只是“宿主可能用得到”的能力，不默认进入 `zom-engine`。

## 3. 阶段与范围

当前阶段、完成度、测试清单和建议命令以 `docs/STATUS.md` 为准。

稳定规则：

```text
改动前确认属于当前 milestone、历史阶段修复，还是后续能力域。
不把多个阶段能力混进一个改动。
不把宿主层能力塞进 engine core。
不为了 examples 或测试污染生产 API。
已收口能力可以修 bug，但不要顺手扩展新能力。
```

## 4. 代码组织

目录职责：

```text
src/        编辑引擎实现，按稳定能力域拆模块
tests/      机器契约测试，CI 主体
examples/   可选交互式 testbed，体感和接入验证
benches/    性能基准
src/tests/  可选内部测试，只测 public API 难覆盖的重要内部不变量
docs/       文档与状态快照
```

`src/lib.rs` 是 public API 门面；目录模块是实现分层，不承诺长期 import path。

模块拆分按能力域，不按行数或临时 milestone。一个文件混合状态、编辑算法、历史、坐标、UI 适配，或 public / pub(crate) 项越来越多且缺少分组时，才考虑拆分。不要为了降低行数制造没有独立语义的新模块。

## 5. Rust 编码风格

优先级：

```text
清晰领域模型 > 局部技巧 > 极致泛型 > 宏抽象
正确性 > 可测试性 > API 清晰度 > 性能
```

强类型：

```text
核心编辑 API 使用 ByteOffset / TextRange。
ByteOffset 是唯一、权威、持久坐标；CharOffset / Utf16Offset / Line / LogicalColumn / DisplayColumn / grapheme cluster 只作为派生坐标或视图坐标。
public API 不裸用 usize 表达领域坐标。
使用 ByteOffset / CharOffset / Utf16Offset / Line / LogicalColumn / DisplayColumn / BufferVersion / TransactionId 等类型表达语义。
```

Public API：

```text
能 private 就不要 pub(crate)。
能 pub(crate) 就不要 pub。
不要为了测试把 private 改成 public。
不要为了 examples 暴露底层细节。
新增 public API 前说明长期语义、调用方影响和测试覆盖。
```

错误处理：

```text
可恢复错误返回明确错误类型。
避免在可恢复路径使用 unwrap / expect / panic。
内部 expect 必须说明已由什么不变量保证。
错误类型应区分坐标、编辑、事务、存储、历史等失败来源。
面向用户、日志或诊断输出的可读提示使用中文；Rust 符号路径、类型名、字段名、协议名等定位信息可保留英文。
```

实现方式：

```text
小步改动。
复用现有 Buffer / Transaction / Selection / Movement / Composition / History 管线。
避免重复实现同一编辑语义。
避免不必要的 clone / to_string / collect。
避免过早抽象和过度 trait 化。
不要为了通过测试临时放宽校验。
```

## 6. 编辑引擎不变量

重点保护：

```text
TextRange: start <= end
Selection: anchor / head 合法，normalized 行为明确
SelectionSet: 排序、合并、不重叠策略明确
EditList: edit 排序、不重叠、非空约束明确
Transaction: base_version 绑定明确
ChangeSet / Delta: 只能由已验证编辑构造
Snapshot: 绑定 BufferVersion，只读，不能伪造过期状态
TransactionRecord / VersionedResult: 版本绑定明确
```

专项检查：

```text
是否混用 ByteOffset / CharOffset / Utf16Offset / Position。
Unicode 边界是否正确：UTF-8、grapheme、word boundary、CRLF。
多光标编辑是否有顺序依赖或重叠问题。
selection after edit 是否由统一映射策略维护。
undo / redo 是否同时恢复文本和 SelectionSet。
composition 是否复用事务管线。
movement 是否复用统一策略，而不是散落在 UI / examples。
history merge 是否属于历史系统，不被宿主输入语义反向塑形。
snapshot 是否保持只读低成本视图，不暴露可变底层。
```

## 7. 关键架构决策

```text
不要恢复 public TextRange::new_unchecked。
Transaction 必须绑定 base_version，Buffer::apply_transaction 必须检查版本。
事务失败必须保持原子性，不部分修改 text / version / dirty / line_index / selection / history / event queue。
生产文本存储使用 RopeyStorage，不自研 Rope / Piece Table / Piece Tree。
StringStorage 只能作为测试 reference model，不放进 src/storage/ 生产核心模块。
Snapshot 应保持只读、低成本、版本绑定，并支持跨线程读取。
examples/ 只服务体感和 API 接入验证，不替代机器契约测试。
M13 之后默认不为每个 engine milestone 新增 GPUI testbed。
```

内部确实需要构造已证明合法的 range 时，优先：

```rust
TextRange::new(start, end).expect("内部不变量: start <= end")
```

## 8. 测试策略

测试放置：

```text
默认 tests/
UI / GPUI / 人类体感 / API 接入手感 -> examples/
性能回归 -> benches/
public API 无法覆盖的重要内部不变量 -> src/tests/
```

`tests/` 验证 public API、阶段契约、错误输入和重构回归；不测 GPUI、窗口、鼠标体感、私有实现细节。

新增测试前确认：能否通过 public API 覆盖？是否保护编辑语义，而不是临时实现细节？失败时是否能定位职责边界或不变量破坏？

修改后通常运行：

```bash
cargo fmt
cargo test
```

定向测试、example check、benchmark 命令以 `docs/STATUS.md` 为准。没有实际运行命令时，必须明确说明。

## 9. 工作模式

审查模式：用户说“看一下 / 检查 / 有没有问题 / 是否符合设计”时，只审查不改代码。结论按风险排序，给证据、影响、建议和未确认事项。

方案模式：用户说“怎么改 / 给方案 / 排计划”时，给最小修改方案，说明目标、文件、步骤、不改范围、测试、风险和验收方式。

实现模式：用户说“实现 / 改掉 / 直接生成补丁 / 文件”时，直接修改。改前说明阶段判断和最小范围；改后说明架构影响、自审结论和验证结果。

## 10. 回答风格

```text
用中文回复。
先说明设计判断，再给具体改法。
区分事实、推测和建议。
指出涉及文件和应运行测试。
不要声称完成了实际没有完成的动作。
没有运行 cargo test，就不要说 cargo test 已通过。
没有检查全仓库，就不要说已经检查了所有文件。
证据不足时，不要断言“没有问题”。
```

## 11. 文档维护

```text
阶段进度、当前文件列表、具体测试清单 -> docs/STATUS.md
能力边界变化 -> docs/编辑引擎能力.md
测试目录职责变化 -> docs/编辑引擎测试策略.md
AI 协作规范变化 -> AGENTS.md
项目概览变化 -> README.md
public API 契约变化 -> 对应测试和必要文档同步更新
```

主文档只维护稳定规则与边界。非原则性变化默认不改主文档。

## 12. 最终原则

```text
zom-engine 是纯文本编辑引擎，不是宿主产品层。
架构质量、职责边界和不变量保护优先于短期功能堆叠。
public API 先稳定语义，不急着暴露内部。
测试以机器契约为主体，examples 只服务体感和接入验证。
所有修改都应能解释：为什么属于 engine core、为什么放在这里、如何验证。
```
