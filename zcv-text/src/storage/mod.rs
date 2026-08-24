//! 文本存储抽象。
//!
//! `TextStorage` 的编辑入口以 `ByteOffset` / `TextRange(byte range)` 为唯一坐标，不假设全文可借出为单段 `&str`；`RopeyStorage` 是唯一后端（有意决策，对齐 Zed text crate 直接绑定 ropey）。
//!
//! `TextRead` / `TextStorage` 是**内部契约**：供 Buffer / Snapshot 门面与测试约束坐标系纪律（字节优先、zero-copy），不代表可插拔存储承诺；
//! 所有生产代码直接以 `RopeyStorage` / `RopeySnapshot` 具体类型持有存储。

mod fingerprint;
mod ropey;
mod traits;

pub(crate) use fingerprint::TextFingerprint;
pub(crate) use ropey::{RopeyPreparedReplace, RopeySnapshot, RopeyStorage};
pub(crate) use traits::{TextRead, TextStorage, text_coordinate_gateway};
