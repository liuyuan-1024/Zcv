//! 文本存储抽象。
//!
//! `RopeyStorage` 的编辑入口以 `ByteOffset` / `TextRange(byte range)` 为唯一坐标，不假设全文可借出为单段 `&str`；它是唯一后端（有意决策，直接绑定 ropey）。
//!
//! `TextRead` 是跨文本算法（如项目搜索）消费只读视图的**跨 crate 协议**，`MultiBufferSnapshot` 实现它；
//! 它同时约束 Buffer / Snapshot 门面的坐标系纪律（字节优先、zero-copy），不代表可插拔存储承诺：
//! `RopeyStorage` 持有当前版本的文本和行索引，负责原子编辑。
//! `Buffer` 借用其只读视图，`Snapshot` 持有写时复制的同版本视图，读取与坐标换算共用 `RopeySnapshot`。

mod line_index;
mod ropey;
mod traits;

pub use line_index::TextSummary;
pub(crate) use ropey::{RopeySnapshot, RopeyStorage};
pub use traits::TextRead;
pub(crate) use traits::text_coordinate_gateway;
