use crossterm::{
    cursor::{self, Hide, MoveTo},
    event::{read, Event::Key, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{self, Clear, ClearType, ScrollUp, SetTitle},
};

use std::io::{self, stdout, Read, Write};

pub struct Editor {
    should_quit: bool,
}

impl Editor {
    pub fn default() -> Self {
        Editor { should_quit: false }
    }

    pub fn run(&mut self) {
        terminal::enable_raw_mode().unwrap();

        loop {
            if let Err(e) = self.refresh_screen() {
                die(e);
            }

            if self.should_quit {
                break;
            }

            if let Err(e) = self.process_keypress() {
                die(e);
            }
        }
    }

    fn refresh_screen(&self) -> Result<(), std::io::Error> {
        execute!(
            stdout(),
            MoveTo(0, 0),
            Clear(ClearType::All),
            SetTitle("Rust Editor"),
        )?;

        if self.should_quit {
            println!("Goodbye, world.\r")
        }

        io::stdout().flush()
    }

    fn process_keypress(&mut self) -> Result<(), std::io::Error> {
        let event = read_key()?;

        match (event.code, event.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                terminal::disable_raw_mode().unwrap();
                self.should_quit = true;
            }
            (KeyCode::Char(c), KeyModifiers::CONTROL) => {
                println!("Ctrl + {}", c);
            }
            (KeyCode::Char(c), KeyModifiers::NONE) => {
                println!("{}", c);
            }
            _ => (),
        }

        Ok(())
    }
}

fn read_key() -> Result<KeyEvent, std::io::Error> {
    loop {
        match read()? {
            Key(event) => return Ok(event),
            _ => (),
        }
    }
}

fn die(e: std::io::Error) {
    println!("{}", e);
    execute!(stdout(), Clear(ClearType::All), cursor::MoveTo(0, 0),).unwrap();

    panic!("Panic");
}
