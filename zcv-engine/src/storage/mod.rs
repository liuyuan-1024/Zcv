//! 文本存储抽象。
//!
//! `TextStorage` 的编辑入口以 `ByteOffset` / `TextRange(byte range)` 为唯一坐标，
//! 不假设全文可借出为单段 `&str`；默认后端是 `RopeyStorage`。

mod fingerprint;
mod ropey;
mod traits;

pub(crate) use fingerprint::TextFingerprint;
pub(crate) use ropey::{RopeySnapshot, RopeyStorage};
pub(crate) use traits::{TextRead, TextStorage, text_coordinate_gateway};
