//! 表单类界面消费统一 Editor 的类型擦除边界。
//!
//! # 契约
//!
//! `EDITOR_FACTORY` 是依赖反转的工厂槽：zcv-ui（设计系统 crate）不依赖任何实现方，由 **zcv_editor::init** 在应用启动时填充（`zcv-editor` 是当前唯一实现方）。
//! 因此：
//! - 任何 host 在使用本模块创建输入框**之前**必须先调用 `zcv_editor::init`；
//! - 工厂未填充时创建输入框会失败——消费方应把输入框视为可缺席能力（`Option`），降级为不渲染输入区，而不是依赖 panic 暴露初始化顺序错误。
//!
//! `ErasedEditor` 是 **Editor 的类型擦除边界**：Zcv 只有一个编辑器实现，本 trait 面向"不能直接依赖 zcv-editor 的 crate"暴露其输入能力面——当前实现为 `Editor::single_line`，只承诺文本读写、占位符、焦点与编辑事件。
//! 装配层（zcv 二进制）可直接依赖 `zcv_editor::Editor`，不需要经过本边界（如版本控制面板的多行提交信息框）。

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
