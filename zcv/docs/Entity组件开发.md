# Entity 组件开发流程

> 基于 zcv 现有组件实践（TopBar、BottomBar、ProjectTree、AppView）提炼的标准流程。
> 新 Entity 组件一律按此流程推进，不做推测性扩展。

---

## 一、文件组织规则

```
src/workbench/xxx/
  mod.rs            ← 公开接口、struct 定义、Render 实现、action handler
  types.rs          ← 数据模型定义（可选：类型较多时抽出）
  controller.rs     ← 状态控制器（可选：状态逻辑复杂时抽出）
  render.rs         ← 渲染子函数（可选：渲染代码较长时抽出）
```

- 组件代码不超过 200 行就**不拆文件**，全放在 `mod.rs` 里
- 不预先拆分，只有当你发现文件确实太长时才拆
- 单一消费方的组件直接内嵌到消费者目录下（参考高内聚原则）

---

## 二、开发八步流程

### 第 1 步：定义 Action

组件有键盘交互时才定义 action。

```rust
// 在组件文件顶部
use gpui::actions;

actions!(
    my_component,                // 命名空间，与模块名对应
    [ActionA, ActionB, ActionC,] // 组件定义的全部 action
);
```

如果组件没有键盘交互，跳过这步。

---

### 第 2 步：定义 Struct 与 constructor

```rust
pub(crate) struct MyComponent {
    focus: gpui::FocusHandle,        // 需要键盘焦点时保留
    state: Rc<RefCell<MyState>>,     // 需要内部可变状态时选用
    child: Entity<ChildComponent>,   // 持有的子 Entity
}

impl MyComponent {
    /// 构造组件。初始参数简单的只传 cx；根级组件可在参数中传入必要数据。
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        // 1. 获得焦点句柄（带焦点的组件必须调用）
        let focus = cx.focus_handle();

        // 2. 创建子 Entity
        let child = cx.new(|cx| ChildComponent::new(cx));

        Self { focus, state: Rc::new(RefCell::new(MyState::new())), child }
    }
}
```

**规则：**
- `new` 的参数，精简为 `cx` 加最少必要参数。根级组件可加初始数据参数（如 `ProjectTree::new(root, cx)`）
- `FocusHandle` 必须在 `new` 中通过 `cx.focus_handle()` 获取，不能延迟
- 快捷键不在 `new` 中注册，统一走 `keymap.rs` + JSON（见快捷键注册说明）
- 不需要焦点时不声明 `focus` 字段

---

### 第 3 步：实现 Render

```rust
impl Render for MyComponent {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("my-component")                   // 稳定 ID，便于调试
            .when_some(self.focus.as_ref(), |el, f| el.track_focus(f))  // 需要焦点时启用
            .when_some(keyboard_context, |el, ctx| el.key_context(ctx)) // 需要键盘上下文时设置
            .size_full()
            .on_action(cx.listener(Self::handle_action_a))
            .on_action(cx.listener(Self::handle_action_b))
            .child(self.child.clone())             // Entity 通过 .clone() 传入
    }
}
```

**规则：**
- `.track_focus()` 只在需要键盘事件的组件上调用
- `.key_context()` 只在需要作用域化快捷键绑定时设置
- 不需要焦点的组件跳过 `track_focus` 和 `key_context`，`focus` 字段不声明

---

### 第 4 步：实现 Action handler

Entity 的 action handler 用 `cx.listener(Self::method)` 绑定：

```rust
impl MyComponent {
    fn handle_action_a(
        &mut self,
        _: &ActionA,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 修改自身状态
        // 调用 window.refresh() 触发重绘
        window.refresh();
    }
}
```

自由函数 handler 用于不需要访问 `&mut self`、只操作全局状态的场景：

```rust
pub(crate) fn handle_global_action(_: &Action, _: &mut Window, cx: &mut gpui::App) {
    if let Some(global) = cx.try_global::<SomeGlobal>() {
        // 通过全局访问共享状态
    }
}
```

**选择：**

| 绑定方式 | 适用场景 |
|---|---|
| `cx.listener(Self::method)` | handler 需要访问 `&mut self` |
| 自由函数 `fn(...)` | handler 只通过全局或 window API 操作 |

---

### 第 5 步：选择状态管理模式

**模式 A：字段直存**（最简单，状态只在本组件内使用）

```rust
struct MyComponent {
    count: u32,
    items: Vec<String>,
}
```

**模式 B：Rc<RefCell<State>>**（内部可变，且状态被多处回调/闭包共用）

```rust
struct ProjectTree {
    state: Rc<RefCell<ProjectTreeState>>,
}

impl Render for ProjectTree {
    fn render(&mut self, ...) -> impl IntoElement {
        let len = self.state.borrow().visible_rows().len();
        render_list(&self.state, len)  // 把 Rc clone 传入闭包
    }
}
```

**模式 C：分离 Controller**（状态逻辑复杂，需要独立单元测试）

```
src/workbench/xxx/
  controller.rs    ← MyController，不依赖 GPUI 类型，纯状态变换
  types.rs         ← 数据模型、渲染期快照
  mod.rs           ← Entity 定义，持有 Rc<RefCell<MyController>>
```

Controller 通过 `snapshot()` 产生只读快照供渲染层消费：

```rust
struct LayoutController { ... }

impl LayoutController {
    fn snapshot(&self) -> LayoutSnapshot {
        LayoutSnapshot { ... }
    }
}
```

**选择速查：**

| 模式 | 适用场景 |
|---|---|
| A: 字段直存 | 组件独占的简单状态 |
| B: Rc<RefCell> | 状态在多个闭包/回调中共享 |
| C: Controller + Snapshot | 复杂状态机，需单元测试 |

---

### 第 6 步：跨组件通信 —— Global 模式

当 action handler 需要修改其他组件的状态时，使用 GPUI 全局状态：

```rust
// 定义全局包装类型
pub(crate) struct LayoutRef(pub(crate) Weak<RefCell<LayoutController>>);
impl gpui::Global for LayoutRef {}

// 在 root 组件中注册（AppView::new）
let layout = Rc::new(RefCell::new(LayoutController::new()));
cx.set_global(LayoutRef(Rc::downgrade(&layout)));

// 在任意 action handler 中读取
pub(crate) fn handle_toggle_panel(_: &Action, _: &mut Window, cx: &mut gpui::App) {
    if let Some(layout) = cx.try_global::<LayoutRef>() {
        if let Some(ctrl) = layout.0.upgrade() {
            ctrl.borrow_mut().toggle_panel(PanelId::Xxx);
            window.refresh();
        }
    }
}
```

**适用场景：**
- 底栏 toggle 按钮展开/折叠顶部布局面板
- 状态栏显示编辑器的行号/语言信息
- 命令面板发起命令影响编辑区

---

### 第 7 步：单元测试

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_initial_conditions() {
        // 测试状态逻辑（不依赖 GPUI 渲染环境）
        let ctrl = MyController::new();
        assert!(ctrl.is_valid());
    }

    #[test]
    fn test_state_transition() {
        let mut ctrl = MyController::new();
        ctrl.perform_action();
        assert_eq!(ctrl.snapshot().some_field, expected_value);
    }
}
```

---

### 第 8 步：在父级装配

```rust
// 在 AppView::new 中创建
let my_component = cx.new(|cx| MyComponent::new(cx));

// 在 Render 中装配
fn render(&mut self, ...) -> impl IntoElement {
    div()
        .child(self.my_component.clone())   // Entity 传入
        .child(some_functional_component()) // 函数式组件同行
}
```

---

## 三、完整示例：一个带状态的最小 Entity

```rust
use gpui::{actions, prelude::*, Context, Render, Window};
use std::rc::Rc;
use std::cell::RefCell;

actions!(counter, [Increment, Decrement]);

pub(crate) struct Counter {
    focus: gpui::FocusHandle,
    count: Rc<RefCell<u32>>,
}

impl Counter {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        Self { focus, count: Rc::new(RefCell::new(0)) }
    }

    fn handle_increment(
        &mut self,
        _: &Increment,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        *self.count.borrow_mut() += 1;
        window.refresh();
    }

    fn handle_decrement(
        &mut self,
        _: &Decrement,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        *self.count.borrow_mut() = self.count.borrow().saturating_sub(1);
        window.refresh();
    }
}

impl Render for Counter {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let value = *self.count.borrow();
        div()
            .id("counter")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::handle_increment))
            .on_action(cx.listener(Self::handle_decrement))
            .child(value.to_string())
    }
}
```

---

## 四、快捷键注册说明

快捷键**不**在组件 `new` 中通过 `cx.bind_keys()` 注册，而是统一走集中式 keymap 系统。一个快捷键从定义到生效需完成以下步骤：

### 4.1 在组件中定义 Action

```rust
// 需要键盘交互的组件定义自己的 action
use gpui::actions;
actions!(pane, [CloseTab, NextTab, PrevTab]);
```

### 4.2 在 keymap.rs build 函数中添加映射

```rust
// src/keymap.rs — 把 action 名称字符串映射到 Rust 类型
fn build(keys: &str, action_name: &str, context: Option<&str>) -> Option<KeyBinding> {
    let binding = match action_name {
        // 已有绑定...
        "pane::CloseTab" => KeyBinding::new(keys, CloseTab, context),
        "pane::NextTab"  => KeyBinding::new(keys, NextTab, context),
        "pane::PrevTab"  => KeyBinding::new(keys, PrevTab, context),
        _ => return None,
    };
    Some(binding)
}
```

需要把 action 类型导入 keymap.rs：

```rust
use crate::workbench::{CloseTab, NextTab, PrevTab, ...};
```

### 4.3 在 JSON 文件中定义键位

```json
{
  "context": "Pane",
  "bindings": {
    "cmd-w": "pane::CloseTab",
    "cmd-l": "pane::NextTab",
    "cmd-h": "pane::PrevTab"
  }
}
```

### 4.4 在组件 Render 中绑定 handler

```rust
impl Render for MyComponent {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Pane")                      // 与 JSON 中的 context 对应
            .on_action(cx.listener(Self::handle_next_tab))
    }
}
```

**要点：**
- `key_context("Xxx")` 必须与 JSON 中的 `"context"` 字段匹配，否则快捷键不触发
- Action 名称字符串（`"pane::CloseTab"`）由 `actions!()` 宏根据模块名自动生成，不能写错
- 不需要快捷键的组件跳过以上所有步骤

## 五、决策速查表

| 问题 | 做法 |
|---|---|
| 需要键盘焦点吗？ | 声明 `FocusHandle` + `.track_focus()` |
| 有快捷键吗？ | `actions!()` + `cx.bind_keys()` + `on_action()` |
| handler 需要访问 `&mut self`？ | `cx.listener(Self::method)` |
| handler 只读全局状态？ | 自由函数 `fn(...)` |
| 状态只在本组件内用？ | 直接字段存 |
| 状态被多个闭包共用？ | `Rc<RefCell<T>>` |
| 状态逻辑复杂？ | 分离 Controller |
| 需要跨组件通信？ | `impl Global` 包装 |
| 列表很长（100+ 项）？ | `uniform_list()` |
| 纯视觉无交互？ | 不做 Entity，函数式即可 |

---

## 五、zcv 现有组件验证

| 组件 | 类型 | 焦点 | 快捷键 | 内部状态 | 跨组件通信 |
|---|---|---|---|---|---|
| `TopBar` | Entity | ❌ | ✅ | ❌ | ❌ |
| `BottomBar` | Entity | ❌ | ✅ | ❌ | Global |
| `WindowControls` | 函数式 | ❌ | ✅ | ❌ | ❌ |
| `ProjectTree` | Entity | ✅ | ✅ | Rc<RefCell> | ❌ |
| `AppView` | Entity(root) | ✅ | ✅ | Rc<RefCell> | Global |

开发新 Entity 组件时，先对号入座这张表，确定需要哪些能力，再按八步流程推进。
