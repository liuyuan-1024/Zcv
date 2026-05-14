use gpui::{
    App, Application, Bounds, Context, FocusHandle, IntoElement, Render, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb,
};

pub trait ExampleState: 'static {
    fn facts(&self) -> Vec<String>;

    fn document(&self) -> Option<String> {
        None
    }
}

pub struct ExampleAction<S> {
    pub label: &'static str,
    pub detail: &'static str,
    pub run: fn(&mut S) -> Result<String, String>,
}

pub fn run_interactive_example<S: ExampleState>(
    title: &'static str,
    summary: &'static str,
    state: S,
    actions: Vec<ExampleAction<S>>,
) {
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, gpui::size(px(980.0), px(720.0)), cx);

        let state = Some(state);
        let actions = Some(actions);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |_window, cx| {
                cx.new(|cx| {
                    DomainExample::new(
                        cx,
                        title,
                        summary,
                        state.expect("example state should be opened once"),
                        actions.expect("example actions should be opened once"),
                    )
                })
            },
        )
        .unwrap();

        cx.activate(true);
    });
}

struct DomainExample<S> {
    title: &'static str,
    summary: &'static str,
    state: S,
    actions: Vec<ExampleAction<S>>,
    log: Vec<String>,
    focus_handle: FocusHandle,
}

impl<S: ExampleState> DomainExample<S> {
    fn new(
        cx: &mut Context<Self>,
        title: &'static str,
        summary: &'static str,
        state: S,
        actions: Vec<ExampleAction<S>>,
    ) -> Self {
        Self {
            title,
            summary,
            state,
            actions,
            log: vec!["ready: 点击左侧动作体验该能力域的 public API 接入。".to_string()],
            focus_handle: cx.focus_handle(),
        }
    }

    fn run_action(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(action) = self.actions.get(index) else {
            self.log.push(format!("error: 未找到动作 {}", index));
            cx.notify();
            return;
        };

        match (action.run)(&mut self.state) {
            Ok(message) => self.log.push(format!("ok: {}", message)),
            Err(message) => self.log.push(format!("error: {}", message)),
        }

        if self.log.len() > 8 {
            self.log.remove(0);
        }

        cx.notify();
    }
}

impl<S: ExampleState> Render for DomainExample<S> {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let document = self.state.document();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .bg(rgb(0x18181B))
            .text_color(rgb(0xE4E4E7))
            .p_6()
            .track_focus(&self.focus_handle)
            .tab_index(0)
            .child(
                div()
                    .text_size(px(28.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(self.title),
            )
            .child(
                div()
                    .text_size(px(15.0))
                    .text_color(rgb(0xA1A1AA))
                    .child(self.summary),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .gap_4()
                    .child(self.render_actions(cx))
                    .child(self.render_state(document)),
            )
    }
}

impl<S: ExampleState> DomainExample<S> {
    fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(300.0))
            .flex()
            .flex_col()
            .gap_2()
            .children(self.actions.iter().enumerate().map(|(index, action)| {
                div()
                    .id(("action", index))
                    .p_3()
                    .rounded_md()
                    .bg(rgb(0x27272A))
                    .border_1()
                    .border_color(rgb(0x3F3F46))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0x323238)))
                    .on_click(cx.listener(move |this, _, _, cx| this.run_action(index, cx)))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(action.label),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_size(px(12.0))
                            .text_color(rgb(0xA1A1AA))
                            .child(action.detail),
                    )
            }))
    }

    fn render_state(&self, document: Option<String>) -> impl IntoElement {
        div()
            .id("state-pane")
            .flex_1()
            .flex()
            .flex_col()
            .gap_3()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(self.state.facts().into_iter().map(|fact| {
                        div()
                            .p_3()
                            .rounded_md()
                            .bg(rgb(0x27272A))
                            .border_1()
                            .border_color(rgb(0x3F3F46))
                            .child(fact)
                    })),
            )
            .children(document.map(|text| {
                div()
                    .p_4()
                    .rounded_md()
                    .bg(rgb(0x09090B))
                    .border_1()
                    .border_color(rgb(0x3F3F46))
                    .font_family("Menlo")
                    .line_height(px(21.0))
                    .child(text)
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(self.log.iter().rev().map(|line| {
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(0xA1A1AA))
                            .child(line.clone())
                    })),
            )
    }
}
