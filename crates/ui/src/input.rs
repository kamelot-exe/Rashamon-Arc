//! Input handling — keyboard and mouse.
//!
//! Wraps SDL2 events for the main application loop.

use sdl2::event::Event as SdlEvent;
use sdl2::keyboard::{Keycode, Scancode};
use sdl2::EventPump;
use std::io;

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
    event_pump: EventPump,
    ctrl_pressed: bool,
}

impl InputHandler {
    pub fn new(event_pump: EventPump) -> Result<Self, io::Error> {
        Ok(Self {
            event_pump,
            ctrl_pressed: false,
        })
    }

    /// Poll for input events. Non-blocking.
    /// Returns None if no event is available.
    pub fn poll_event(&mut self) -> Result<Option<Event>, io::Error> {
        match self.event_pump.poll_event() {
            Some(event) => {
                let keyboard_state = self.event_pump.keyboard_state();
                self.ctrl_pressed = keyboard_state.is_scancode_pressed(Scancode::LCtrl)
                    || keyboard_state.is_scancode_pressed(Scancode::RCtrl);

                match event {
                    SdlEvent::Quit { .. } => Ok(Some(Event::Quit)),
                    SdlEvent::KeyDown { keycode: Some(keycode), .. } => {
                        let key = match keycode {
                            Keycode::Escape => Some(Key::Escape),
                            Keycode::Return => Some(Key::Enter),
                            Keycode::Backspace => Some(Key::Backspace),
                            Keycode::Left => Some(Key::Left),
                            Keycode::Right => Some(Key::Right),
                            Keycode::Up => Some(Key::Up),
                            Keycode::Down => Some(Key::Down),
                            Keycode::P => Some(Key::Char('p')),
                            Keycode::T => Some(Key::Char('t')),
                            Keycode::W => Some(Key::Char('w')),
                            Keycode::R => Some(Key::Char('r')),
                            _ => None,
                        };
                        if let Some(k) = key {
                            Ok(Some(Event::KeyPress(k)))
                        } else {
                            Ok(None)
                        }
                    }
                    SdlEvent::TextInput { text, .. } => {
                        if !self.is_ctrl_pressed() {
                            if let Some(c) = text.chars().next() {
                                Ok(Some(Event::KeyPress(Key::Char(c))))
                            } else {
                                Ok(None)
                            }
                        } else {
                            Ok(None)
                        }
                    }
                    SdlEvent::MouseMotion { x, y, .. } => Ok(Some(Event::MouseMove { x, y })),
                    SdlEvent::MouseButtonDown { x, y, mouse_btn, .. } => {
                        Ok(Some(Event::MouseDown { x, y, button: mouse_btn as u8 }))
                    }
                    _ => Ok(None),
                }
            }
            None => Ok(None),
        }
    }

    pub fn is_ctrl_pressed(&self) -> bool {
        self.ctrl_pressed
    }
}
