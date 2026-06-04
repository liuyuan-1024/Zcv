//! 外部 owner 注册表 —— 让 shell runtime 把自己持有的可嵌入编辑器 owner
//! 挂进 App 的路由器。
//!
//! 设计目的：拆解 §2 里"App 直接持有所有 L3 model"的反向依赖。每一个准备
//! 从 App 迁出去的文本 owner（settings TOML 编辑器、文件树新建/重命名输入框、
//! 搜索框、项目选择器查询框……）在 shell runtime 构造时把自己包成
//! `Rc<RefCell<dyn TextTargetOwner>>` 注册进来；App 的 `with_router(_mut)` 在
//! 自家 owner（主编辑区、未迁出的 model）之外把注册表里的 owner 也叠进路由。
//!
//! 之所以用 `Rc<RefCell<>>` 而不是借用：注册时 runtime 仍是该 owner 的真正拥有者，
//! drop / 复用语义都归 runtime；App 只是借一个引用计数做路由。`RefCell` 给写路径
//! 提供运行期借用检查 —— 路由器同帧只在一个 owner 上调可变方法，不会冲突。
//!
//! 本 commit 只引入容器与最朴素的 `register / iter` 接口；focus 细化 / 派发后钩子
//! 等更复杂的扩展点等到真有第一个 owner 需要时再补，以免抽象凭空预设。

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use super::TextTargetOwner;

/// 外部 owner 注册表。App 字段持有；shell runtime 通过 `App::install_editor_owner`
/// 把自己注册进来。
#[derive(Default)]
pub(crate) struct EditorTargetRegistry {
    owners: Vec<Rc<RefCell<dyn TextTargetOwner>>>,
}

impl EditorTargetRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 注册一个 owner。runtime 在 [`ShellView::new`] 阶段一次性调；App 不再
    /// 在自己的 struct 里加新字段。
    ///
    /// 本 commit 只搭骨架，第一个真正注册的 owner 在 commit 3（SettingsTomlEditor 迁移）
    /// 出现 —— 在此之前生产路径无调用，allow 静默 dead_code 警告。
    ///
    /// [`ShellView::new`]: crate::shell::view::ShellView
    #[allow(dead_code)]
    pub(crate) fn register(&mut self, owner: Rc<RefCell<dyn TextTargetOwner>>) {
        self.owners.push(owner);
    }

    /// 当前注册数。给单测断言用。
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.owners.len()
    }

    /// 一次性把所有 owner 借成共享读引用。返回的 `Ref` 列表必须比借出的
    /// `&dyn` 引用活得长 —— 调用方建一个本地变量持有它即可（参考 App 端
    /// `with_router` 的用法）。
    pub(crate) fn borrow_all(&self) -> Vec<Ref<'_, dyn TextTargetOwner>> {
        let mut out = Vec::with_capacity(self.owners.len());
        for rc in &self.owners {
            out.push(rc.borrow());
        }
        out
    }

    /// 一次性把所有 owner 借成可变引用。每个 owner 的 `RefMut` 独立，写路径
    /// 同帧只会落到 `accepts_focus` 命中的那一个，借用冲突由 router 行为而非
    /// 类型系统保证。
    ///
    /// `+ 'static` 显式标注：`RefMut<'_, T>` 对 `T` 不变（invariant），不允许编译期
    /// 把存储里的 `dyn TextTargetOwner + 'static` 收窄成调用处推出的更短 dyn 寿命。
    pub(crate) fn borrow_all_mut(&self) -> Vec<RefMut<'_, dyn TextTargetOwner + 'static>> {
        let mut out = Vec::with_capacity(self.owners.len());
        for rc in &self.owners {
            out.push(rc.borrow_mut());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    //! 注册表本身的契约。与 App 的集成测试在 `app::tests` 里另起一条。
    use super::*;
    use crate::focus::AppFocus;
    use crate::shell::editor::{EditorSnapshot, ImeQueryTarget, ImeTarget};
    use zom_command::{EditTarget, KeyContext};

    /// 一个最小桩 owner —— 不持任何真实文本，仅按构造时给定的 focus 表态 accept。
    struct StubOwner {
        focus: AppFocus,
        flag: bool,
    }

    impl StubOwner {
        fn new(focus: AppFocus) -> Self {
            Self { focus, flag: false }
        }
    }

    impl crate::shell::editor::TextTargetQuery for StubOwner {
        fn accepts_focus(&self, focus: AppFocus) -> bool {
            focus == self.focus
        }
        fn snapshot(&self) -> EditorSnapshot {
            EditorSnapshot::default()
        }
        fn key_contexts(&self) -> Vec<KeyContext> {
            vec![KeyContext::global()]
        }
        fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
            None
        }
    }

    impl crate::shell::editor::TextTargetOwner for StubOwner {
        fn ime_target(&mut self) -> Option<ImeTarget<'_>> {
            None
        }
        fn edit_target(&mut self) -> Option<EditTarget<'_>> {
            None
        }
        fn after_text_changed(&mut self) {
            self.flag = true;
        }
    }

    #[test]
    fn registered_owners_appear_in_iteration_order() {
        let mut registry = EditorTargetRegistry::new();
        let a: Rc<RefCell<dyn TextTargetOwner>> =
            Rc::new(RefCell::new(StubOwner::new(AppFocus::editor())));
        let b: Rc<RefCell<dyn TextTargetOwner>> =
            Rc::new(RefCell::new(StubOwner::new(AppFocus::settings())));
        registry.register(Rc::clone(&a));
        registry.register(Rc::clone(&b));
        assert_eq!(registry.len(), 2);

        let borrows = registry.borrow_all();
        assert!(borrows[0].accepts_focus(AppFocus::editor()));
        assert!(borrows[1].accepts_focus(AppFocus::settings()));
    }

    #[test]
    fn borrow_all_mut_can_invoke_writes_independently() {
        let mut registry = EditorTargetRegistry::new();
        let owner = Rc::new(RefCell::new(StubOwner::new(AppFocus::editor())));
        let owner_for_dyn: Rc<RefCell<dyn TextTargetOwner>> = owner.clone();
        registry.register(owner_for_dyn);

        {
            let mut borrows = registry.borrow_all_mut();
            borrows[0].after_text_changed();
        }
        assert!(
            owner.borrow().flag,
            "after_text_changed 应当作用到具体 owner"
        );
    }
}
