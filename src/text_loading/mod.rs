//! 文件文本加载边界：定义 UTF-8 bytes 进入 Buffer 时保留下来的编码与文本形态元信息。
//!
//! 本模块只描述加载结果和策略词汇，不执行 I/O、不保存文件，也不参与 reload 冲突处理。

mod encoding;
mod loaded_text;

pub use encoding::{BomPolicy, InvalidUtf8Policy, TextEncoding};
pub use loaded_text::LoadedTextInfo;
