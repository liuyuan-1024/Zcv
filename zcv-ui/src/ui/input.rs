//! 表单类界面消费统一 Editor 的类型擦除边界。

use std::sync::{Arc, OnceLock};

use gpui::{AnyElement, App, FocusHandle, Subscription, Window};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErasedEditorEvent {
    Edited,
}

pub type ErasedEditorEventHandler =
    Box<dyn FnMut(ErasedEditorEvent, &mut Window, &mut App) + 'static>;
pub type ErasedEditorFactory = fn(&mut App) -> Arc<dyn ErasedEditor>;

pub trait ErasedEditor: 'static {
    fn text(&self, cx: &App) -> String;
    fn set_text(&self, text: &str, cx: &mut App);
    fn set_placeholder_text(&self, text: &str, cx: &mut App);
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn subscribe(
        &self,
        callback: ErasedEditorEventHandler,
        window: &mut Window,
        cx: &mut App,
    ) -> Subscription;
    fn render(&self) -> AnyElement;
}

pub static EDITOR_FACTORY: OnceLock<ErasedEditorFactory> = OnceLock::new();
