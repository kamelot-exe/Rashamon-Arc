//! Input handling — keyboard and mouse.

#[cfg(feature = "linux-desktop")]
use sdl2::event::Event as SdlEvent;
#[cfg(feature = "linux-desktop")]
use sdl2::keyboard::Scancode;
#[cfg(feature = "linux-desktop")]
use sdl2::EventPump;
use std::io;

#[derive(Clone, Copy, Debug, Default)]
#[allow(dead_code)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Other(u8),
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum PlatformEvent {
    Quit,
    Tick,
    KeyDown {
        key: BrowserKey,
        modifiers: Modifiers,
    },
    TextInput(String),
    MouseMove {
        x: i32,
        y: i32,
    },
    MouseDown {
        x: i32,
        y: i32,
        button: MouseButton,
    },
    MouseUp {
        x: i32,
        y: i32,
        button: MouseButton,
    },
    /// Vertical scroll wheel delta: positive = scroll up (toward page top).
    Scroll {
        delta: i32,
    },
    WindowResized {
        width: u32,
        height: u32,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum BrowserKey {
    Escape,
    Enter,
    Backspace,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Char(char),
}

#[cfg(feature = "linux-desktop")]
pub struct InputHandler {
    event_pump: EventPump,
    modifiers: Modifiers,
}

#[cfg(feature = "linux-desktop")]
impl InputHandler {
    pub fn new(event_pump: EventPump) -> Result<Self, io::Error> {
        Ok(Self {
            event_pump,
            modifiers: Modifiers::default(),
        })
    }

    /// Poll one *recognised* event, draining and discarding unrecognised SDL
    /// events (WindowEvent, FocusGained, Expose, etc.) along the way.
    /// Returns None only when the event queue is truly empty.
    pub fn poll_event(&mut self) -> Result<Option<PlatformEvent>, io::Error> {
        loop {
            // sdl2 0.35.2 panics on unknown/extended keycodes (e.g. 0x435).
            // catch_unwind lets us discard those events without crashing.
            let raw = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.event_pump.poll_event()
            })) {
                Ok(None) => return Ok(None), // queue empty
                Ok(Some(e)) => e,
                Err(_) => continue, // unknown keycode — skip event
            };

            // Keep modifier state fresh on every SDL event.
            let ks = self.event_pump.keyboard_state();
            self.modifiers = Modifiers {
                ctrl: ks.is_scancode_pressed(Scancode::LCtrl) || ks.is_scancode_pressed(Scancode::RCtrl),
                shift: ks.is_scancode_pressed(Scancode::LShift)
                    || ks.is_scancode_pressed(Scancode::RShift),
                alt: ks.is_scancode_pressed(Scancode::LAlt) || ks.is_scancode_pressed(Scancode::RAlt),
                meta: ks.is_scancode_pressed(Scancode::LGui) || ks.is_scancode_pressed(Scancode::RGui),
            };

            let recognised = match raw {
                SdlEvent::Quit { .. } => Some(PlatformEvent::Quit),

                SdlEvent::KeyDown {
                    scancode: Some(sc), ..
                } => {
                    let key = match sc {
                        Scancode::Escape => Some(BrowserKey::Escape),
                        Scancode::Return => Some(BrowserKey::Enter),
                        Scancode::KpEnter => Some(BrowserKey::Enter),
                        Scancode::Backspace => Some(BrowserKey::Backspace),
                        Scancode::Left => Some(BrowserKey::Left),
                        Scancode::Right => Some(BrowserKey::Right),
                        Scancode::Up => Some(BrowserKey::Up),
                        Scancode::Down => Some(BrowserKey::Down),
                        Scancode::PageUp => Some(BrowserKey::PageUp),
                        Scancode::PageDown => Some(BrowserKey::PageDown),
                        // Ctrl+shortcuts — captured here so they work even
                        // when SDL text-input mode is active.
                        Scancode::T if self.modifiers.ctrl => Some(BrowserKey::Char('t')),
                        Scancode::W if self.modifiers.ctrl => Some(BrowserKey::Char('w')),
                        Scancode::R if self.modifiers.ctrl => Some(BrowserKey::Char('r')),
                        Scancode::S if self.modifiers.ctrl && self.modifiers.shift => Some(BrowserKey::Char('s')),
                        Scancode::P if self.modifiers.ctrl => Some(BrowserKey::Char('p')),
                        Scancode::H if self.modifiers.ctrl => Some(BrowserKey::Char('h')),
                        Scancode::B if self.modifiers.ctrl => Some(BrowserKey::Char('b')),
                        Scancode::F if self.modifiers.ctrl => Some(BrowserKey::Char('f')),
                        Scancode::I if self.modifiers.ctrl => Some(BrowserKey::Char('i')),
                        Scancode::L if self.modifiers.ctrl => Some(BrowserKey::Char('l')),
                        Scancode::N if self.modifiers.ctrl => Some(BrowserKey::Char('n')),
                        Scancode::Equals if self.modifiers.ctrl => Some(BrowserKey::ZoomIn),
                        Scancode::KpPlus if self.modifiers.ctrl => Some(BrowserKey::ZoomIn),
                        Scancode::Minus if self.modifiers.ctrl => Some(BrowserKey::ZoomOut),
                        Scancode::KpMinus if self.modifiers.ctrl => Some(BrowserKey::ZoomOut),
                        Scancode::Num0 if self.modifiers.ctrl => Some(BrowserKey::ZoomReset),
                        _ => None,
                    };
                    key.map(|key| PlatformEvent::KeyDown {
                        key,
                        modifiers: self.modifiers,
                    })
                }

                // TextInput fires for printable characters when text input is
                // active (SDL_StartTextInput was called).  Skip while Ctrl held
                // so shortcuts don't also type a letter.
                SdlEvent::TextInput { text, .. } if !self.modifiers.ctrl => {
                    Some(PlatformEvent::TextInput(text))
                },

                SdlEvent::MouseMotion { x, y, .. } => Some(PlatformEvent::MouseMove { x, y }),

                SdlEvent::MouseButtonDown {
                    x, y, mouse_btn, ..
                } => Some(PlatformEvent::MouseDown {
                    x,
                    y,
                    button: map_mouse_button(mouse_btn as u8),
                }),

                SdlEvent::MouseButtonUp {
                    x, y, mouse_btn, ..
                } => Some(PlatformEvent::MouseUp {
                    x,
                    y,
                    button: map_mouse_button(mouse_btn as u8),
                }),

                SdlEvent::MouseWheel { y, .. } => Some(PlatformEvent::Scroll { delta: y }),

                SdlEvent::Window {
                    win_event: sdl2::event::WindowEvent::Resized(width, height),
                    ..
                } => Some(PlatformEvent::WindowResized {
                    width: width.max(0) as u32,
                    height: height.max(0) as u32,
                }),

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
}

#[cfg(feature = "linux-desktop")]
fn map_mouse_button(button: u8) -> MouseButton {
    match button {
        1 => MouseButton::Left,
        2 => MouseButton::Middle,
        3 => MouseButton::Right,
        other => MouseButton::Other(other),
    }
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
pub struct InputHandler {
    modifiers: Modifiers,
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
impl InputHandler {
    pub fn new_kamelot_stub() -> Result<Self, io::Error> {
        Ok(Self {
            modifiers: Modifiers::default(),
        })
    }

    pub fn poll_event(&mut self) -> Result<Option<PlatformEvent>, io::Error> {
        Ok(None)
    }
}
