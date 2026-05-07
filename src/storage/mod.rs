//! 文本存储抽象。
//!
//! M3.5 起，TextStorage 的编辑入口使用 CharOffset / TextRange(char range)，
//! M4 起默认后端切换为 RopeyStorage，并且 TextStorage 不再假设全文可作为连续 &str 借出。

mod fingerprint;
mod ropey;
mod traits;

pub(crate) use fingerprint::TextFingerprint;
pub(crate) use ropey::{RopeySnapshot, RopeyStorage};
pub(crate) use traits::{TextRead, TextStorage};
