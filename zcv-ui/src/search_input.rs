//! 搜索输入框:通用输入框外壳 + 匹配选项、匹配导航按钮与命中计数的现成组合。
//!
//! 内部插槽固定插入 3 个匹配选项(区分大小写 / 整词匹配 / 正则表达式);
//! 外部插槽固定插入「上一个匹配 / 下一个匹配」导航按钮与命中计数(计数在导航之后);
//! 经 [`SearchInput::external`] 可在计数之后追加调用方自选的按钮(如「替换」开关,是否插入由调用方决定)。

use std::rc::Rc;

use gpui::{
    Action, AnyElement, App, IntoElement, ParentElement, SharedString, Window, div, prelude::*,
};
use zcv_actions::{FindNext, FindPrevious, ToggleCaseSensitive, ToggleRegex, ToggleWholeWord};
use zcv_theme::{color, typography};

use crate::button::Button;
use crate::input_shell::InputShell;

/// 匹配选项的当前状态,决定选项按钮是否高亮;会话侧可直接以它为状态字段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MatchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

impl MatchOptions {
    /// 翻转指定选项,返回新状态(不可变,便于与组件/回调组合)。
    pub fn toggled(self, option: MatchOption) -> Self {
        match option {
            MatchOption::CaseSensitive => Self {
                case_sensitive: !self.case_sensitive,
                ..self
            },
            MatchOption::WholeWord => Self {
                whole_word: !self.whole_word,
                ..self
            },
            MatchOption::Regex => Self {
                regex: !self.regex,
                ..self
            },
        }
    }
}

/// 匹配选项种类;切换回调据此区分被切换的选项。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchOption {
    CaseSensitive,
    WholeWord,
    Regex,
}

impl MatchOption {
    const ALL: [MatchOption; 3] = [
        MatchOption::CaseSensitive,
        MatchOption::WholeWord,
        MatchOption::Regex,
    ];

    fn id(self, prefix: &str) -> String {
        match self {
            MatchOption::CaseSensitive => format!("{prefix}-case-sensitive"),
            MatchOption::WholeWord => format!("{prefix}-whole-word"),
            MatchOption::Regex => format!("{prefix}-regex"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            MatchOption::CaseSensitive => "icons/case_sensitive.svg",
            MatchOption::WholeWord => "icons/whole_word.svg",
            MatchOption::Regex => "icons/regex.svg",
        }
    }

    fn label(self) -> &'static str {
        match self {
            MatchOption::CaseSensitive => "区分大小写",
            MatchOption::WholeWord => "整词匹配",
            MatchOption::Regex => "正则表达式",
        }
    }

    fn shortcut(self) -> &'static dyn Action {
        match self {
            MatchOption::CaseSensitive => &ToggleCaseSensitive,
            MatchOption::WholeWord => &ToggleWholeWord,
            MatchOption::Regex => &ToggleRegex,
        }
    }

    fn active(self, options: MatchOptions) -> bool {
        match self {
            MatchOption::CaseSensitive => options.case_sensitive,
            MatchOption::WholeWord => options.whole_word,
            MatchOption::Regex => options.regex,
        }
    }
}

/// 匹配选项切换回调。
type ToggleHandler = Rc<dyn Fn(MatchOption, &mut Window, &mut App)>;
/// 一次动作回调(导航 / 计数等)。
type ActionHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 搜索输入框。
pub struct SearchInput {
    id_prefix: SharedString,
    input: AnyElement,
    options: MatchOptions,
    toggle: Option<ToggleHandler>,
    /// (当前匹配的显示序号, 命中总数)。
    count: Option<(usize, usize)>,
    previous: Option<ActionHandler>,
    next: Option<ActionHandler>,
    external: Vec<AnyElement>,
}

impl SearchInput {
    /// `id_prefix` 保证同屏多个搜索输入框的元素 id 不冲突(如 `"buffer-search"` / `"project-search"`)。
    pub fn new(id_prefix: impl Into<SharedString>, input: impl Into<AnyElement>) -> Self {
        Self {
            id_prefix: id_prefix.into(),
            input: input.into(),
            options: MatchOptions::default(),
            toggle: None,
            count: None,
            previous: None,
            next: None,
            external: Vec::new(),
        }
    }

    /// 匹配选项的激活状态。
    pub fn options(mut self, options: MatchOptions) -> Self {
        self.options = options;
        self
    }

    /// 点击匹配选项时的回调;参数携带被点击的选项。
    pub fn on_toggle(
        mut self,
        handler: impl Fn(MatchOption, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.toggle = Some(Rc::new(handler));
        self
    }

    /// 命中计数:传入当前活动匹配(0-based,无活动匹配为 `None`)与命中总数,组件渲染 `"当前/总数"` 文案并自行决定颜色(无命中时为占位色);
    /// 渲染在导航按钮之后。
    pub fn count(mut self, active: Option<usize>, total: usize) -> Self {
        self.count = Some((active.map_or(0, |index| index + 1), total));
        self
    }

    /// 跳到上一个匹配。
    pub fn on_previous(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.previous = Some(Rc::new(handler));
        self
    }

    /// 跳到下一个匹配。
    pub fn on_next(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.next = Some(Rc::new(handler));
        self
    }

    /// 追加外部插槽:渲染在计数与导航按钮之后(调用方自选,如「替换」开关)。
    pub fn external(mut self, element: impl IntoElement) -> Self {
        self.external.push(element.into_any_element());
        self
    }
}

impl IntoElement for SearchInput {
    type Element = gpui::ViewElement<Self>;

    fn into_element(self) -> Self::Element {
        gpui::ViewElement::new(self)
    }
}

impl RenderOnce for SearchInput {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = color::current(cx);
        let prefix = self.id_prefix.as_ref();
        let mut shell = InputShell::new(self.input);
        for option in MatchOption::ALL {
            if let Some(toggle) = &self.toggle {
                let handler = toggle.clone();
                shell = shell.internal(
                    Button::icon(option.id(prefix), option.icon())
                        .label(option.label())
                        .shortcut(option.shortcut(), cx)
                        .color(if option.active(self.options) {
                            colors.icon_accent
                        } else {
                            colors.text_muted
                        })
                        .on_click(move |_, window, cx| handler(option, window, cx)),
                );
            }
        }
        if let Some(previous) = self.previous {
            shell = shell.external(
                Button::icon(format!("{prefix}-previous"), "icons/chevron_left.svg")
                    .label("上一个匹配")
                    .shortcut(&FindPrevious, cx)
                    .on_click(move |_, window, cx| previous(window, cx)),
            );
        }
        if let Some(next) = self.next {
            shell = shell.external(
                Button::icon(format!("{prefix}-next"), "icons/chevron_right.svg")
                    .label("下一个匹配")
                    .shortcut(&FindNext, cx)
                    .on_click(move |_, window, cx| next(window, cx)),
            );
        }
        if let Some((current, total)) = self.count {
            shell = shell.external(
                div()
                    .text_color(if total > 0 {
                        colors.text_muted
                    } else {
                        colors.text_placeholder
                    })
                    .text_size(typography::ui_size())
                    .child(format!("{current}/{total}")),
            );
        }
        for element in self.external {
            shell = shell.external(element);
        }
        shell
    }
}
