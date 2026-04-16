//! Input handling — keyboard and mouse.

use sdl2::event::Event as SdlEvent;
use sdl2::keyboard::Scancode;
use sdl2::EventPump;
use std::io;

#[derive(Debug)]
pub enum Event {
    Quit,
    KeyPress(Key),
    MouseMove { x: i32, y: i32 },
    MouseDown { x: i32, y: i32, button: u8 },
}

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

pub struct InputHandler {
    event_pump: EventPump,
    ctrl_pressed: bool,
}

impl InputHandler {
    pub fn new(event_pump: EventPump) -> Result<Self, io::Error> {
        Ok(Self { event_pump, ctrl_pressed: false })
    }

    /// Poll one *recognised* event, draining and discarding unrecognised SDL
    /// events (WindowEvent, FocusGained, Expose, etc.) along the way.
    /// Returns None only when the event queue is truly empty.
    pub fn poll_event(&mut self) -> Result<Option<Event>, io::Error> {
        loop {
            let raw = match self.event_pump.poll_event() {
                None => return Ok(None), // queue empty
                Some(e) => e,
            };

            // Keep ctrl state fresh on every SDL event.
            let ks = self.event_pump.keyboard_state();
            self.ctrl_pressed = ks.is_scancode_pressed(Scancode::LCtrl)
                || ks.is_scancode_pressed(Scancode::RCtrl);

            let recognised = match raw {
                SdlEvent::Quit { .. } => Some(Event::Quit),

                SdlEvent::KeyDown { scancode: Some(sc), .. } => {
                    let key = match sc {
                        Scancode::Escape    => Some(Key::Escape),
                        Scancode::Return    => Some(Key::Enter),
                        Scancode::KpEnter   => Some(Key::Enter),
                        Scancode::Backspace => Some(Key::Backspace),
                        Scancode::Left      => Some(Key::Left),
                        Scancode::Right     => Some(Key::Right),
                        Scancode::Up        => Some(Key::Up),
                        Scancode::Down      => Some(Key::Down),
                        // Ctrl+shortcuts — captured here so they work even
                        // when SDL text-input mode is active.
                        Scancode::T if self.ctrl_pressed => Some(Key::Char('t')),
                        Scancode::W if self.ctrl_pressed => Some(Key::Char('w')),
                        Scancode::R if self.ctrl_pressed => Some(Key::Char('r')),
                        Scancode::P if self.ctrl_pressed => Some(Key::Char('p')),
                        _ => None,
                    };
                    key.map(Event::KeyPress)
                }

                // TextInput fires for printable characters when text input is
                // active (SDL_StartTextInput was called).  Skip while Ctrl held
                // so shortcuts don't also type a letter.
                SdlEvent::TextInput { text, .. } if !self.ctrl_pressed => {
                    text.chars().next().map(|c| Event::KeyPress(Key::Char(c)))
                }

                SdlEvent::MouseMotion { x, y, .. } => Some(Event::MouseMove { x, y }),

                SdlEvent::MouseButtonDown { x, y, mouse_btn, .. } => {
                    Some(Event::MouseDown { x, y, button: mouse_btn as u8 })
                }

                // All other SDL events (WindowEvent, FocusGained, Exposed …)
                // are silently consumed; the loop continues to the next event.
                _ => None,
            };

            if let Some(ev) = recognised {
                return Ok(Some(ev));
            }
            // else: unrecognised event discarded, try next in queue
        }
    }

    pub fn is_ctrl_pressed(&self) -> bool { self.ctrl_pressed }
}
