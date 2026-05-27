//! 文本目标层。

mod id;
mod owned;
mod router;
mod traits;

pub(crate) use id::{TextTargetId, TextTargetIds};
pub(crate) use owned::OwnedEditorTarget;
pub(crate) use router::{EditorRouter, EditorRouterMut};
pub(crate) use traits::{TextTargetOwner, TextTargetQuery};
