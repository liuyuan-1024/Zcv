use std::collections::HashSet;

use gpui::TestAppContext;

use super::commands;

#[gpui::test]
fn described_commands_have_unique_buildable_actions(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let mut names = HashSet::new();
        for command in commands() {
            assert!(
                names.insert(command.action_name),
                "重复的命令：{}",
                command.action_name
            );
            assert!(
                !command.title.trim().is_empty(),
                "命令标题为空：{}",
                command.action_name
            );
            assert!(
                !command.description.trim().is_empty(),
                "命令说明为空：{}",
                command.action_name
            );
            assert!(
                cx.build_action(command.action_name, None).is_ok(),
                "命令 action 无法构造：{}",
                command.action_name
            );
        }
    });
}
