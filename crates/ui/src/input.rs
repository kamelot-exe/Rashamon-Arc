//! Input handling — keyboard and mouse.
//!
//! Wraps SDL2 events for the main application loop.

use sdl2::event::Event as SdlEvent;
use sdl2::keyboard::{Keycode, Mod, Scancode};
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
                            Keycode::Escape => Key::Escape,
                            Keycode::Return => Key::Enter,
                            Keycode::Backspace => Key::Backspace,
                            Keycode::Left => Key::Left,
                            Keycode::Right => Key::Right,
                            Keycode::Up => Key::Up,
                            Keycode::Down => Key::Down,
                            // Simple char conversion for now
                            Keycode::A => Key::Char('a'),
                            Keycode::B => Key::Char('b'),
                            Keycode::C => Key::Char('c'),
                            Keycode::D => Key::Char('d'),
                            Keycode::E => Key::Char('e'),
                            Keycode::F => Key::Char('f'),
                            Keycode::G => Key::Char('g'),
                            Keycode::H => Key::Char('h'),
                            Keycode::I => Key::Char('i'),
                            Keycode::J => Key::Char('j'),
                            Keycode::K => Key::Char('k'),
                            Keycode::L => Key::Char('l'),
                            Keycode::M => Key::Char('m'),
                            Keycode::N => Key::Char('n'),
                            Keycode::O => Key::Char('o'),
                            Keycode::P => Key::Char('p'),
                            Keycode::Q => Key::Char('q'),
                            Keycode::R => Key::Char('r'),
                            Keycode::S => Key::Char('s'),
                            Keycode::T => Key::Char('t'),
                            Keycode::U => Key::Char('u'),
                            Keycode::V => Key::Char('v'),
                            Keycode::W => Key::Char('w'),
                            Keycode::X => Key::Char('x'),
                            Keycode::Y => Key::Char('y'),
                            Keycode::Z => Key::Char('z'),
                            Keycode::Num0 => Key::Char('0'),
                            Keycode::Num1 => Key::Char('1'),
                            Keycode::Num2 => Key::Char('2'),
                            Keycode::Num3 => Key::Char('3'),
                            Keycode::Num4 => Key::Char('4'),
                            Keycode::Num5 => Key::Char('5'),
                            Keycode::Num6 => Key::Char('6'),
                            Keycode::Num7 => Key::Char('7'),
                            Keycode::Num8 => Key::Char('8'),
                            Keycode::Num9 => Key::Char('9'),
                            Keycode::Space => Key::Char(' '),
                            _ => return Ok(None), // Ignore other keys for now
                        };
                        Ok(Some(Event::KeyPress(key)))
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
