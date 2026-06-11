//! 编辑文本目标协议。
//!
//! 这里放 app / shell 共享的非 GPUI 文本能力：IME 目标、文本快照、自持输入框。
//! shell 的 editor 视图负责把这些快照画出来；app 只通过这些纯协议做路由。

mod ime;
mod owned_target;
pub(crate) mod snapshot;

pub(crate) use ime::{ImeQueryTarget, ImeUtf16Range};
pub(crate) use owned_target::OwnedEditorTarget;
pub(crate) use snapshot::{EditorSnapshot, EditorSnapshotRequest, RevealHint, build_snapshot};
