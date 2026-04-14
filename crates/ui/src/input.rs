//! Input handling — keyboard and mouse from Linux evdev.
//!
//! For the MVP, reads from stdin as a fallback.
//! In production, uses evdev / DRM input subsystem.

use std::io::{self, Read, stdin};

/// Input events.
#[derive(Debug)]
pub enum Event {
    Quit,
    KeyPress(Key),
    MouseMove { x: i32, y: i32 },
    MouseDown { x: i32, y: i32, button: u8 },
}

/// Keyboard keys we care about.
#[derive(Debug)]
pub enum Key {
    Escape,
    Enter,
    Backspace,
    Left,
    Right,
    Up,
    Down,
    Char(char),
}

/// Input handler.
pub struct InputHandler {
    buffer: String,
}

impl InputHandler {
    pub fn new() -> Result<Self, io::Error> {
        eprintln!("[input] stdin input mode (evdev in production)");
        Ok(Self {
            buffer: String::new(),
        })
    }

    /// Poll for input events. Non-blocking.
    /// Returns None if no event is available.
    pub fn poll_event(&mut self) -> Result<Option<Event>, io::Error> {
        // Stub: return None every frame.
        // In production, reads from /dev/input/eventX via evdev.
        Ok(None)
    }
}

/// Blocking input helper — reads a single keypress.
pub fn read_key_blocking() -> Result<Key, io::Error> {
    let mut buf = [0u8; 1];
    stdin().read_exact(&mut buf)?;
    
    match buf[0] {
        b'\x1b' => Ok(Key::Escape),
        b'\n' | b'\r' => Ok(Key::Enter),
        b'\x7f' | b'\x08' => Ok(Key::Backspace),
        c if c >= 32 && c < 127 => Ok(Key::Char(c as char)),
        _ => Ok(Key::Char('?')),
    }
}
