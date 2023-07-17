use crossterm::event::{KeyCode, KeyEvent};

use crate::{Mode, PossibleModes};

enum Command {
    Save,
    Quit,
}

enum Direction {
    Forward,
    Backward,
    Down,
    Up,
}

enum MovementType {
    Word,
    Line,
    Character,
}

enum Count {
    Number(u32),
    Infinity,
}

pub enum Action {
    Move(MovementType, Direction, Count),
    Delete,
    Yank,
    Change,
}

pub fn convert_keypress_to_action(event: KeyEvent, current_mode: &Mode) -> Option<Action> {
    // match current_mode {
    //     // Normal mode
    //     //
    //     // Insert mode

    //     // Visual mode

    //     // Operator mode
    //     _ => None,
    // };
    let mut action: Action = Action::Move(
        MovementType::Character,
        Direction::Forward,
        Count::Number(0),
    );

    // Keys that work in both modes
    match (event.code, event.modifiers) {
        (KeyCode::Up, _) => {
            action = Action::Move(MovementType::Character, Direction::Up, Count::Number(1));
        }
        // (KeyCode::Up, _)
        // | (KeyCode::Down, _)
        // | (KeyCode::Left, _)
        // | (KeyCode::Right, _)
        // | (KeyCode::PageUp, _)
        // | (KeyCode::PageDown, _)
        // | (KeyCode::Home, _)
        // | (KeyCode::End, _) => Some(Action::Move(
        //     MovementType::Character,
        //     Direction::Forward,
        //     Count::Number(1),
        // )),
        _ => {}
    }

    Some(action)
}
