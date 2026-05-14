use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommandId(String);

impl CommandId {
    pub fn new(value: impl Into<String>) -> Result<Self, CommandError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(CommandError::InvalidCommandId);
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Command {
    pub id: CommandId,
    pub title: String,
}

impl Command {
    pub fn new(id: CommandId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CommandRegistry {
    commands: BTreeMap<CommandId, Command>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, command: Command) -> Result<(), CommandError> {
        if self.commands.contains_key(&command.id) {
            return Err(CommandError::DuplicateCommand(command.id));
        }

        self.commands.insert(command.id.clone(), command);
        Ok(())
    }

    pub fn get(&self, id: &CommandId) -> Option<&Command> {
        self.commands.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Command> {
        self.commands.values()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyBinding {
    pub chord: KeyChord,
    pub command: CommandId,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct KeyChord(String);

impl KeyChord {
    pub fn new(value: impl Into<String>) -> Result<Self, CommandError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(CommandError::InvalidKeyChord);
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Default)]
pub struct Keymap {
    bindings: BTreeMap<KeyChord, CommandId>,
}

impl Keymap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(&mut self, chord: KeyChord, command: CommandId) {
        self.bindings.insert(chord, command);
    }

    pub fn resolve(&self, chord: &KeyChord) -> Option<&CommandId> {
        self.bindings.get(chord)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum CommandError {
    DuplicateCommand(CommandId),
    InvalidCommandId,
    InvalidKeyChord,
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateCommand(id) => write!(f, "命令已注册: {id}"),
            Self::InvalidCommandId => f.write_str("命令 id 不能为空"),
            Self::InvalidKeyChord => f.write_str("快捷键不能为空"),
        }
    }
}

impl std::error::Error for CommandError {}
