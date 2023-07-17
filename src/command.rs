use std::collections::HashMap;

pub struct Command {
    pub name: String,
    pub aliases: Vec<String>,
    pub args: Vec<String>,
}

impl Command {
    pub fn new(name: String, aliases: Vec<String>, args: Vec<String>) -> Self {
        Self {
            name,
            aliases,
            args,
        }
    }
}

pub struct Commands {
    pub commands: HashMap<String, Command>,
}

impl Commands {
    pub fn new() -> Self {
        let mut commands = HashMap::new();

        commands.insert(
            "quit".to_string(),
            Command::new("quit".to_string(), vec!["q".to_string()], vec![]),
        );

        commands.insert(
            "save".to_string(),
            Command::new("save".to_string(), vec!["w".to_string()], vec![]),
        );

        Self { commands }
    }
}

pub enum CommandResult {
    Quit,
    Save,
    None,
}
