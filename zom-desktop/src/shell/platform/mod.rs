//! shell::platform —— 平台差异收纳层（手册 14.x）。
//!
//! 第一版骨架阶段：本模块只占位，承诺所有平台分支都将集中在此一层。
//! 其它 L2 / L3 组件**不允许** `#[cfg(target_os = "...")]`（手册 14.1）。
//!
//! 后续在此填充的子模块（按手册 14.x）：
//!   - `keyboard`：OS key event → 归一化 `KeyChord`（14.2）
//!   - `keymap_logical`：逻辑修饰键 ↔ 实际键映射（14.3）
//!   - `paths`：配置 / 缓存 / 日志目录（12.2 / 17.8）
//!   - `signals`：SIGINT / SIGTERM / 平台 quit 事件（25.5）
//!   - `ime`：OS IME 事件适配（19.6）
//!   - `window`：窗口控制（最小化 / 放大 / 关闭）（14.6）

pub(crate) mod window;
