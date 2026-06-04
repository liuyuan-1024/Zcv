//! 文本目标层。

mod embedded;
mod owned;
mod registry;
mod router;
mod traits;

pub(crate) use embedded::EmbeddedEditorTarget;
pub(crate) use owned::OwnedEditorTarget;
pub(crate) use registry::EditorTargetRegistry;
pub(crate) use router::{EditorRouter, EditorRouterMut};
pub(crate) use traits::{TextTargetOwner, TextTargetQuery};
