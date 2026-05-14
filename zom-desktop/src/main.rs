use zom_command::{Command, CommandId, CommandRegistry};
use zom_workspace::Workspace;

fn main() {
    let mut commands = CommandRegistry::new();
    register_builtin_commands(&mut commands);

    let mut workspace = Workspace::new();
    let buffer_id = workspace
        .open_text(None, "")
        .expect("内部不变量: 空文本应能创建默认 Buffer");

    println!(
        "zom-desktop initialized: {} commands, active buffer {}",
        commands.iter().count(),
        buffer_id.as_u64()
    );
}

fn register_builtin_commands(commands: &mut CommandRegistry) {
    let builtins = [
        ("file.open", "打开文件"),
        ("file.save", "保存文件"),
        ("command.palette.open", "打开命令面板"),
        ("editor.undo", "撤销"),
        ("editor.redo", "重做"),
        ("ai.edit_selection", "AI 修改选区"),
    ];

    for (id, title) in builtins {
        let id = CommandId::new(id).expect("内部不变量: builtin command id 非空");
        commands
            .register(Command::new(id, title))
            .expect("内部不变量: builtin command id 唯一");
    }
}
