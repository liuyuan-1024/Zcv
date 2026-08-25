# Builder 组件开发流程

> 基于当前项目中 `Button` 组件的实践提炼的标准。
> Builder 组件是设计系统的原子 UI 原语——封装可配置的交互性标记，供全项目跨模块复用。

---

## 一、定义与定位

**Builder 组件 = 封装交互性 UI 原语的构建器。**

- 通过 struct 持有构建期配置参数
- 通过链式方法（builder pattern）设置可选选项
- 最终通过 `IntoElement` 转换为 GPUI 元素
- 封装**交互约定**（tooltip 视觉风格、点击回调签名、悬停行为等）

### 与函数式组件和 Entity 组件的区别

| 维度 | 函数式组件 | Builder 组件 | Entity 组件 |
|---|---|---|---|
| 核心 | 消除布局重复 | 封装交互原语 | 封装生命周期与状态 |
| struct 定义 | 不需要 | 需要 | 需要 |
| 实现 trait | 不需要 | 实现 `IntoElement` | 实现 `Render` |
| 内部状态 | 无 | 构建期持有配置 | 跨帧持有状态 |
| 终结方式 | 返回 `Div` 等具体类型 | `IntoElement`，调用方直接做子元素 | `impl IntoElement` |
| 调用方用法 | `组件.child(fn(...))` | `组件.child(Button::icon(...))` | `组件.child(entity.clone())` |
| 复用范围 | 单模块或 shared | 全项目 | 按 Entity 定位 |
| 典型规模 | 1–10 行 | 80–200 行 | 200+ 行 |

**判断准则：** 这坨代码解决的是"长得一样"还是"行为一样"？

- **长得一样** → Div 结构重复 → **函数式组件**
- **行为一样且参数少** → 重建交互模式，参数固定 → **函数式组件**（如 `pip(enum, bool)`）
- **行为一样且参数组合多** → 有默认值、可选链、多个消费方 → **Builder 组件**

---

## 二、开发三步流程

### 第 1 步：定义内容变体

组件支持哪几种内容形态？用枚举表达：

```rust
#[derive(Clone)]
enum ButtonContent {
    Icon(&'static str),                    // 纯图标
    Text(String),                          // 纯文字
    IconText { icon: &'static str, text: String },  // 图标 + 文字
}
```

**规则：**
- 内容变体枚举是组件的核心抽象，定义在 struct 之前
- 只列出支持的内容形态，不预留未来变体
- 每个变体在外部对应一个工厂构造函数

---

### 第 2 步：定义 Struct、工厂构造函数与 Builder 方法

```rust
pub struct Button {
    id: ElementId,                                      // 必需：稳定标识
    content: ButtonContent,                             // 必需：内容变体
    style: ButtonStyle,                                 // 视觉样式（Ghost 默认）
    color: Option<gpui::Rgba>,                          // 可选，默认延迟到 render 解析
    tooltip: TooltipSpec,                               // 可选：label + 快捷键
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>, // 可选
    disabled: bool,                                     // 可选：禁用交互
}

impl Button {
    // 工厂构造函数——直接确定变体，不暴露 content 字段
    pub fn icon(id: impl Into<ElementId>, path: &'static str) -> Self {
        Self::new(id, ButtonContent::Icon(path))
    }

    pub fn text(id: impl Into<ElementId>, text: impl Into<String>) -> Self {
        Self::new(id, ButtonContent::Text(text.into()))
    }

    pub fn icon_text(
        id: impl Into<ElementId>,
        path: &'static str,
        text: impl Into<String>,
    ) -> Self {
        Self::new(id, ButtonContent::IconText { icon: path, text: text.into() })
    }

    // 私有构造函数：设置默认值
    fn new(id: impl Into<ElementId>, content: ButtonContent) -> Self {
        Self {
            id: id.into(),
            content,
            style: ButtonStyle::Ghost,
            color: None,
            tooltip: TooltipSpec::default(),
            on_click: None,
            disabled: false,
        }
    }

    // ── Builder 方法 ──

    /// 设置视觉样式。
    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
        self
    }

    /// 覆写默认颜色。
    pub fn color(mut self, color: gpui::Rgba) -> Self {
        self.color = Some(color);
        self
    }

    /// 设置悬停 tooltip 文案（需配合快捷键时可再调 shortcut）。
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.tooltip = TooltipSpec::new(label);
        self
    }

    /// 设置点击回调。
    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// 禁用交互，但保留图形作为可见的操作反馈。
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}
```

**规则：**
- 工厂构造函数明确表达"这是在创建什么"（icon/text/icon_text）
- 私有构造函数统一设置默认值，不暴露
- 所有可选字段用 `Option` 类型，默认为 `None`
- 如没有多种变体，直接公开 `new()` 构造函数代替工厂方法
- 每个 builder 方法只做一件事——例：不该在 `color()` 里顺便设置 `tooltip`
- 方法签名 = `mut self, 具体参数 -> Self`（消费所有权，返回所有权）
- 参数类型用 `impl Into<T>` 让调用方方便
- 回调参数签名与 GPUI 约定一致（`&mut Window, &mut App`）
- 回调需要 `Rc` 包装的，在 builder 方法内部处理，调用方不感知

---

### 第 3 步：终结——实现 IntoElement

Builder 组件的最后一个环节是实现 `IntoElement`，让 builder 表达式本身可以直接作为 GPUI 子元素使用。

```rust
impl IntoElement for Button {
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // 容器：按样式选择视觉外壳（Ghost 悬停显背景 / Solid 常驻边框背景）。
        let mut element = match self.style {
            ButtonStyle::Ghost => div().id(self.id).rounded_sm().p(space::S2),
            ButtonStyle::Solid => div()
                .id(self.id)
                .px(space::S12).py(space::S6)
                .rounded_md()
                .border_1().border_color(colors.border_variant)
                .bg(colors.panel_background),
        };
        // 公共交互注入：手型光标、tooltip、悬停背景、点击回调。
        // 内容分支：图标经 SvgIcon 渲染，文字直接 div。
        element.child(content)
    }
}
```

**规则：**
- `type Element = gpui::AnyElement;`——直接擦除类型，与 GPUI 原生元素一致
- 内容变体的分支在 `match` 中展开，每个分支独立处理
- 公共行为（tooltip、click）提取到 `apply_base` 闭包，避免分支内重复
- 如果组件需要子元素（entity / 回调渲染），在构造函数或 builder 方法中传入

---

## 三、tooltip 与快捷键

悬停提示统一复用 `zcv-ui::TooltipSpec`：组件持有规格（label + 可选快捷键文本），
在 `IntoElement` 中 `tooltip.build()` 构建气泡。无需自定义内部 Entity：

```rust
// Button 中通过 label()/shortcut() 组装规格，render 时挂载。
if let Some(build) = self.tooltip.build() {
    element = element.tooltip(build);
}
```

**规则：**
- 组件持有 `TooltipSpec` 而非视图，悬停时才构建气泡 Entity
- 快捷键文本由 `TooltipSpec::with_action` 从 keymap 预查询，不依赖悬停时机
- 纯展示组件（如 `SvgIcon`）同样复用该机制，但 tooltip 依赖 stateful 元素，需先设置 `id`

---

## 四、完整示例：一个最小 Builder 组件

```rust
// my_badge.rs
use gpui::{div, prelude::*, AnyElement, ElementId, IntoElement, Window, App};
use std::rc::Rc;

enum BadgeContent {
    Text(String),
    Count(u32),
}

pub struct Badge {
    id: ElementId,
    content: BadgeContent,
    color: Option<gpui::Rgba>,
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl Badge {
    pub fn text(id: impl Into<ElementId>, text: impl Into<String>) -> Self {
        Self::new(id, BadgeContent::Text(text.into()))
    }

    pub fn count(id: impl Into<ElementId>, count: u32) -> Self {
        Self::new(id, BadgeContent::Count(count))
    }

    fn new(id: impl Into<ElementId>, content: BadgeContent) -> Self {
        Self { id: id.into(), content, color: None, on_click: None }
    }

    pub fn color(mut self, color: gpui::Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl IntoElement for Badge {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        let color = self.color;
        let on_click = self.on_click;

        let mut el = div()
            .id(self.id)
            .px(space::S4).py(space::S2)
            .rounded(radius::FULL)
            .border_1().border_color(color)
            .text_color(color);

        if let Some(ref handler) = on_click {
            let h = Rc::clone(handler);
            el = el.on_click(move |_, window, cx| h(window, cx));
        }

        let inner = match self.content {
            BadgeContent::Text(t) => div().child(t).into_any_element(),
            BadgeContent::Count(n) => div().child(n.to_string()).into_any_element(),
        };

        el.child(inner).into_any_element()
    }
}
```

---

## 五、调用方式

```rust
// 方法一：直接作为子元素（IntoElement 自动转换）
div().child(
    Button::icon("my-button", "icons/action.svg")
        .label("操作")
        .on_click(|_, _, _| println!("点击")),
)

// 方法二：放入 Vec<AnyElement> 集合时，用 .into_any_element()
div().children(vec![
    Button::icon("btn-a", "icons/a.svg").label("A").into_any_element(),
    Button::icon("btn-b", "icons/b.svg").label("B").into_any_element(),
])
```

两种用法都与 GPUI 原生元素（`div()`、`svg()`）完全一致。

---

## 六、决策：何时做 Builder 组件

| 条件 | 结论 |
|---|---|
| 只有 1 个变体、1 个消费方、无交互 | 不做 Builder，直接写 Div |
| 1 个变体、1 个消费方、有 tooltip/click | 函数式返回 `Stateful<Div>` 即可 |
| 多个变体（icon/text/icon_text）、0–2 个可选配置 | 函数式 + 枚举参数 |
| **多个变体、3+ 个可选配置、2+ 消费方** | **Builder 组件** |
| 变体固定、但可选配置持续增长到 6+ | Builder 组件（已到 Button 水准） |

---

## 七、现有 Builder 组件

| 组件 | 位置 | 内容变体 | 可选配置 |
|---|---|---|---|
| `Button` | `ui/button.rs` | icon / text / icon_text | style, color, label, shortcut, on_click, disabled |
| `SvgIcon` | `ui/icon.rs` | icon | color, size, id, label, shortcut |

---

## 八、快速检查清单

- [ ] 有 ≥ 2 个真实消费方？（否则先做函数式组件）
- [ ] 内容形态用枚举表达了？
- [ ] 工厂构造函数明确了每种变体？
- [ ] 所有可选字段有默认值？
- [ ] 每个 builder 方法只做一件事？
- [ ] 回调签名是 `&mut Window, &mut App`？
- [ ] `IntoElement` 把公共交互行为提取出去了？
- [ ] `IntoElement` 的 `type Element = AnyElement`？
