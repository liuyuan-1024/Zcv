//! 替换输入框:通用输入框外壳 + 替换操作按钮的现成组合。
//!
//! 外部插槽固定插入「替换当前匹配」与「替换全部匹配」两个按钮。

use std::rc::Rc;

use gpui::{AnyElement, App, SharedString, Window, prelude::*};
use zcv_actions::{ReplaceAll, ReplaceNext};

use crate::button::Button;
use crate::input_shell::InputShell;

/// 替换动作回调。
type ActionHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 替换输入框。
pub struct ReplaceInput {
    id_prefix: SharedString,
    input: AnyElement,
    replace: Option<ActionHandler>,
    replace_all: Option<ActionHandler>,
}

impl ReplaceInput {
    /// `id_prefix` 保证同屏多个替换输入框的元素 id 不冲突(如 `"buffer-replace"`)。
    pub fn new(id_prefix: impl Into<SharedString>, input: impl Into<AnyElement>) -> Self {
        Self {
            id_prefix: id_prefix.into(),
            input: input.into(),
            replace: None,
            replace_all: None,
        }
    }

    /// 替换当前匹配(并前进到下一个匹配)。
    pub fn on_replace(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.replace = Some(Rc::new(handler));
        self
    }

    /// 替换全部匹配。
    pub fn on_replace_all(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.replace_all = Some(Rc::new(handler));
        self
    }
}

impl IntoElement for ReplaceInput {
    type Element = gpui::ViewElement<Self>;

    fn into_element(self) -> Self::Element {
        gpui::ViewElement::new(self)
    }
}

impl RenderOnce for ReplaceInput {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let prefix = self.id_prefix.as_ref();
        let mut shell = InputShell::new(self.input);
        if let Some(replace) = self.replace {
            shell = shell.external(
                Button::icon(format!("{prefix}-replace"), "icons/replace_next.svg")
                    .label("替换")
                    .shortcut(&ReplaceNext, cx)
                    .on_click(move |_, window, cx| replace(window, cx)),
            );
        }
        if let Some(replace_all) = self.replace_all {
            shell = shell.external(
                Button::icon(format!("{prefix}-replace-all"), "icons/replace_all.svg")
                    .label("全部替换")
                    .shortcut(&ReplaceAll, cx)
                    .on_click(move |_, window, cx| replace_all(window, cx)),
            );
        }
        shell
    }
}
