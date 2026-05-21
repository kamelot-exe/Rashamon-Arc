//! Platform runtime loop boundary.
//!
//! Runtime backends own platform display/input/framebuffer details. The browser
//! loop consumes normalized `PlatformEvent`s and asks the runtime to present the
//! already-rendered framebuffer.

use crate::input::PlatformEvent;
#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
use crate::input::{BrowserKey, Modifiers, MouseButton};
use crate::layout::{FB_HEIGHT, FB_WIDTH};
use rashamon_renderer::{CursorKind, Framebuffer};
#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
use std::collections::VecDeque;
use std::io;
use std::time::Duration;
#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
use std::time::Instant;

#[allow(dead_code)]
pub(crate) trait PlatformRuntime {
    fn poll_events(&mut self) -> io::Result<Vec<PlatformEvent>>;
    fn framebuffer_mut(&mut self) -> &mut Framebuffer;
    fn present_frame(&mut self) -> io::Result<()>;
    fn window_size(&self) -> (u32, u32);
    fn request_redraw(&mut self) {}
    fn should_exit(&self) -> bool {
        false
    }
    fn tick(&mut self) {
        std::thread::sleep(Duration::from_millis(16));
    }
    fn set_cursor(&mut self, _cursor: CursorKind) {}
}

#[cfg(feature = "linux-desktop")]
#[allow(dead_code)]
pub(crate) struct LinuxDesktopRuntime {
    display: crate::display::Display,
    input: crate::input::InputHandler,
    framebuffer: Framebuffer,
    scale_x: f32,
    scale_y: f32,
    win_w: u32,
    win_h: u32,
    current_cursor: CursorKind,
}

#[cfg(feature = "linux-desktop")]
impl LinuxDesktopRuntime {
    pub(crate) fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let sdl = sdl2::init()?;
        let video = sdl.video()?;
        let _ = sdl.mouse().show_cursor(true);
        video.text_input().start();

        let (win_w, win_h) = video
            .current_display_mode(0)
            .map(|m| (m.w as u32, m.h as u32))
            .unwrap_or((FB_WIDTH, FB_HEIGHT));
        let win_w = win_w.min(FB_WIDTH);
        let win_h = win_h.min(FB_HEIGHT);
        let scale_x = FB_WIDTH as f32 / win_w as f32;
        let scale_y = FB_HEIGHT as f32 / win_h as f32;

        let event_pump = sdl.event_pump()?;
        let framebuffer = Framebuffer::new(FB_WIDTH, FB_HEIGHT);
        let display = crate::display::Display::new(&video, win_w, win_h, FB_WIDTH, FB_HEIGHT)?;
        let input = crate::input::InputHandler::new(event_pump)?;

        Ok(Self {
            display,
            input,
            framebuffer,
            scale_x,
            scale_y,
            win_w,
            win_h,
            current_cursor: CursorKind::Default,
        })
    }

    fn scale_event(&self, event: PlatformEvent) -> PlatformEvent {
        match event {
            PlatformEvent::MouseMove { x, y } => PlatformEvent::MouseMove {
                x: scale(x, self.scale_x, FB_WIDTH) as i32,
                y: scale(y, self.scale_y, FB_HEIGHT) as i32,
            },
            PlatformEvent::MouseDown { x, y, button } => PlatformEvent::MouseDown {
                x: scale(x, self.scale_x, FB_WIDTH) as i32,
                y: scale(y, self.scale_y, FB_HEIGHT) as i32,
                button,
            },
            PlatformEvent::MouseUp { x, y, button } => PlatformEvent::MouseUp {
                x: scale(x, self.scale_x, FB_WIDTH) as i32,
                y: scale(y, self.scale_y, FB_HEIGHT) as i32,
                button,
            },
            other => other,
        }
    }
}

#[cfg(feature = "linux-desktop")]
impl PlatformRuntime for LinuxDesktopRuntime {
    fn poll_events(&mut self) -> io::Result<Vec<PlatformEvent>> {
        let mut events = Vec::new();
        while let Some(event) = self.input.poll_event()? {
            events.push(self.scale_event(event));
        }
        Ok(events)
    }

    fn framebuffer_mut(&mut self) -> &mut Framebuffer {
        &mut self.framebuffer
    }

    fn present_frame(&mut self) -> io::Result<()> {
        self.display.present(&self.framebuffer)
    }

    fn window_size(&self) -> (u32, u32) {
        (self.win_w, self.win_h)
    }

    fn set_cursor(&mut self, cursor: CursorKind) {
        if self.current_cursor == cursor {
            return;
        }
        use sdl2::mouse::{Cursor, SystemCursor};
        let system = match cursor {
            CursorKind::Default => SystemCursor::Arrow,
            CursorKind::Pointer => SystemCursor::Hand,
            CursorKind::Text => SystemCursor::IBeam,
            CursorKind::Wait => SystemCursor::Wait,
        };
        if let Ok(cursor_handle) = Cursor::from_system(system) {
            cursor_handle.set();
            self.current_cursor = cursor;
        }
    }
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
pub(crate) struct KamelotRuntime {
    framebuffer: Framebuffer,
    scanout: KamelotScanout,
    input: KamelotSyscallInputSource,
    event_queue: VecDeque<PlatformEvent>,
    mouse_x: i32,
    mouse_y: i32,
    width: u32,
    height: u32,
    redraw_requested: bool,
    debug: bool,
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
impl KamelotRuntime {
    pub(crate) fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let framebuffer = Framebuffer::new(FB_WIDTH, FB_HEIGHT);
        let debug = std::env::var_os("RASHAMON_DEBUG").is_some();
        let scanout =
            KamelotScanout::new(framebuffer.width, framebuffer.height, framebuffer.stride);
        if debug {
            eprintln!(
                "[runtime:kamelot] selected framebuffer={}x{} stride={} bytes={}",
                framebuffer.width,
                framebuffer.height,
                framebuffer.stride,
                framebuffer.data.len()
            );
        }
        Ok(Self {
            framebuffer,
            scanout,
            input: KamelotSyscallInputSource::new(debug),
            event_queue: VecDeque::new(),
            mouse_x: 0,
            mouse_y: 0,
            width: FB_WIDTH,
            height: FB_HEIGHT,
            redraw_requested: true,
            debug,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn push_event(&mut self, event: PlatformEvent) {
        self.event_queue.push_back(event);
    }

    #[allow(dead_code)]
    pub(crate) fn presented_frame(&self) -> &[u8] {
        self.scanout.front_buffer()
    }

    #[allow(dead_code)]
    pub(crate) fn frame_count(&self) -> u64 {
        self.scanout.present_count()
    }

    #[allow(dead_code)]
    pub(crate) fn inject_input_event(&mut self, event: KamelotInputEvent) {
        self.input.inject_event(event);
    }

    fn poll_input_source(&mut self) -> io::Result<()> {
        for event in self.input.poll_input()? {
            if self.debug {
                eprintln!("[runtime:kamelot] input {:?}", event);
            }
            if let Some(platform_event) = self.map_input_event(event) {
                self.event_queue.push_back(platform_event);
            }
        }
        Ok(())
    }

    fn map_input_event(&mut self, event: KamelotInputEvent) -> Option<PlatformEvent> {
        match event {
            KamelotInputEvent::Quit => Some(PlatformEvent::Quit),
            KamelotInputEvent::KeyDown { key, modifiers } => {
                Some(PlatformEvent::KeyDown { key, modifiers })
            }
            KamelotInputEvent::Text(text) => Some(PlatformEvent::TextInput(text)),
            KamelotInputEvent::MouseMove { dx, dy } => {
                self.mouse_x = (self.mouse_x + dx).clamp(0, self.width.saturating_sub(1) as i32);
                self.mouse_y = (self.mouse_y + dy).clamp(0, self.height.saturating_sub(1) as i32);
                Some(PlatformEvent::MouseMove {
                    x: self.mouse_x,
                    y: self.mouse_y,
                })
            }
            KamelotInputEvent::MousePosition { x, y } => {
                self.mouse_x = x.clamp(0, self.width.saturating_sub(1) as i32);
                self.mouse_y = y.clamp(0, self.height.saturating_sub(1) as i32);
                Some(PlatformEvent::MouseMove {
                    x: self.mouse_x,
                    y: self.mouse_y,
                })
            }
            KamelotInputEvent::MouseButton { button, pressed } => {
                if pressed {
                    Some(PlatformEvent::MouseDown {
                        x: self.mouse_x,
                        y: self.mouse_y,
                        button,
                    })
                } else {
                    Some(PlatformEvent::MouseUp {
                        x: self.mouse_x,
                        y: self.mouse_y,
                        button,
                    })
                }
            }
            KamelotInputEvent::Scroll { delta } => Some(PlatformEvent::Scroll { delta }),
        }
    }
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
impl PlatformRuntime for KamelotRuntime {
    fn poll_events(&mut self) -> io::Result<Vec<PlatformEvent>> {
        self.poll_input_source()?;
        Ok(self.event_queue.drain(..).collect())
    }

    fn framebuffer_mut(&mut self) -> &mut Framebuffer {
        &mut self.framebuffer
    }

    fn present_frame(&mut self) -> io::Result<()> {
        self.scanout.present(&self.framebuffer);
        self.redraw_requested = false;
        if self.debug && self.scanout.present_count() == 1 {
            eprintln!(
                "[runtime:kamelot] present frame={} bytes={} elapsed_ms={}",
                self.scanout.present_count(),
                self.scanout.front_buffer().len(),
                self.scanout.last_present_elapsed_ms().unwrap_or(0)
            );
        }
        Ok(())
    }

    fn window_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn request_redraw(&mut self) {
        self.redraw_requested = true;
    }

    fn should_exit(&self) -> bool {
        self.scanout.present_count() > 0 && self.event_queue.is_empty() && !self.redraw_requested
    }

    fn tick(&mut self) {
        // Future Kamelot syscall timer hook. Host simulation yields a stable
        // 60-ish Hz cadence without depending on SDL or window APIs.
        std::thread::sleep(Duration::from_millis(16));
    }
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
#[derive(Debug)]
pub(crate) enum KamelotInputEvent {
    Quit,
    KeyDown {
        key: BrowserKey,
        modifiers: Modifiers,
    },
    Text(String),
    MouseMove {
        dx: i32,
        dy: i32,
    },
    MousePosition {
        x: i32,
        y: i32,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
    },
    Scroll {
        delta: i32,
    },
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
struct KamelotSyscallInputSource {
    pending: VecDeque<KamelotInputEvent>,
    debug: bool,
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
impl KamelotSyscallInputSource {
    fn new(debug: bool) -> Self {
        Self {
            pending: VecDeque::new(),
            debug,
        }
    }

    fn poll_input(&mut self) -> io::Result<Vec<KamelotInputEvent>> {
        self.poll_syscalls()?;
        Ok(self.pending.drain(..).collect())
    }

    #[allow(dead_code)]
    fn inject_event(&mut self, event: KamelotInputEvent) {
        self.pending.push_back(event);
    }

    fn poll_syscalls(&mut self) -> io::Result<()> {
        // Future Kamelot syscall hook:
        // - read keyboard packets
        // - read mouse packets
        // - translate them into KamelotInputEvent
        //
        // Host builds keep this empty so the Kamelot feature remains
        // compile-only without depending on SDL, GTK, WebKitGTK, or Linux
        // device APIs.
        if self.debug {
            // Keep diagnostics opt-in and low-volume.
        }
        Ok(())
    }
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
struct KamelotScanout {
    memory: KamelotFramebufferMemory,
    present_count: u64,
    last_present: Option<Instant>,
    last_present_elapsed: Option<Duration>,
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
impl KamelotScanout {
    fn new(width: u32, height: u32, stride: u32) -> Self {
        Self {
            memory: KamelotFramebufferMemory::owned(width, height, stride),
            present_count: 0,
            last_present: None,
            last_present_elapsed: None,
        }
    }

    fn present(&mut self, source: &Framebuffer) {
        self.memory.copy_from(source);
        self.present_count = self.present_count.saturating_add(1);
        let now = Instant::now();
        self.last_present_elapsed = self
            .last_present
            .map(|last| now.saturating_duration_since(last));
        self.last_present = Some(now);
    }

    fn front_buffer(&self) -> &[u8] {
        &self.memory.bytes
    }

    fn present_count(&self) -> u64 {
        self.present_count
    }

    fn last_present_elapsed_ms(&self) -> Option<u128> {
        self.last_present_elapsed.map(|elapsed| elapsed.as_millis())
    }
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
struct KamelotFramebufferMemory {
    width: u32,
    height: u32,
    stride: u32,
    bytes: Vec<u8>,
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
impl KamelotFramebufferMemory {
    fn owned(width: u32, height: u32, stride: u32) -> Self {
        Self {
            width,
            height,
            stride,
            bytes: vec![0u8; (stride * height) as usize],
        }
    }

    fn copy_from(&mut self, source: &Framebuffer) {
        debug_assert_eq!(self.width, source.width);
        debug_assert_eq!(self.height, source.height);
        debug_assert_eq!(self.stride, source.stride);
        self.bytes.copy_from_slice(&source.data);
    }
}

#[inline]
fn scale(v: i32, factor: f32, max: u32) -> u32 {
    ((v.max(0) as f32 * factor) as u32).min(max - 1)
}
