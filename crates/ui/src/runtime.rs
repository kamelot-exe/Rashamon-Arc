//! Platform runtime loop boundary.
//!
//! Runtime backends own platform display/input/framebuffer details. The browser
//! loop consumes normalized `PlatformEvent`s and asks the runtime to present the
//! already-rendered framebuffer.

use crate::input::PlatformEvent;
use crate::layout::{FB_HEIGHT, FB_WIDTH};
use rashamon_renderer::{CursorKind, Framebuffer};
#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
use std::collections::VecDeque;
use std::io;
use std::time::Duration;

#[allow(dead_code)]
pub(crate) trait PlatformRuntime {
    fn poll_events(&mut self) -> io::Result<Vec<PlatformEvent>>;
    fn framebuffer_mut(&mut self) -> &mut Framebuffer;
    fn present_frame(&mut self) -> io::Result<()>;
    fn window_size(&self) -> (u32, u32);
    fn request_redraw(&mut self) {}
    fn should_exit(&self) -> bool { false }
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
    front_buffer: Vec<u8>,
    event_queue: VecDeque<PlatformEvent>,
    width: u32,
    height: u32,
    frame_count: u64,
    redraw_requested: bool,
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
impl KamelotRuntime {
    pub(crate) fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let framebuffer = Framebuffer::new(FB_WIDTH, FB_HEIGHT);
        let front_buffer = vec![0u8; framebuffer.data.len()];
        if std::env::var_os("RASHAMON_DEBUG").is_some() {
            eprintln!(
                "[runtime:kamelot] framebuffer {}x{} stride={} bytes={}",
                framebuffer.width,
                framebuffer.height,
                framebuffer.stride,
                framebuffer.data.len()
            );
        }
        Ok(Self {
            framebuffer,
            front_buffer,
            event_queue: VecDeque::new(),
            width: FB_WIDTH,
            height: FB_HEIGHT,
            frame_count: 0,
            redraw_requested: true,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn push_event(&mut self, event: PlatformEvent) {
        self.event_queue.push_back(event);
    }

    #[allow(dead_code)]
    pub(crate) fn presented_frame(&self) -> &[u8] {
        &self.front_buffer
    }

    #[allow(dead_code)]
    pub(crate) fn frame_count(&self) -> u64 {
        self.frame_count
    }
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
impl PlatformRuntime for KamelotRuntime {
    fn poll_events(&mut self) -> io::Result<Vec<PlatformEvent>> {
        Ok(self.event_queue.drain(..).collect())
    }

    fn framebuffer_mut(&mut self) -> &mut Framebuffer {
        &mut self.framebuffer
    }

    fn present_frame(&mut self) -> io::Result<()> {
        self.front_buffer.copy_from_slice(&self.framebuffer.data);
        self.frame_count = self.frame_count.saturating_add(1);
        self.redraw_requested = false;
        if std::env::var_os("RASHAMON_DEBUG").is_some() && self.frame_count == 1 {
            eprintln!(
                "[runtime:kamelot] present frame={} bytes={}",
                self.frame_count,
                self.front_buffer.len()
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
        self.frame_count > 0 && self.event_queue.is_empty() && !self.redraw_requested
    }

    fn tick(&mut self) {
        // Future Kamelot syscall timer hook. Host simulation yields a stable
        // 60-ish Hz cadence without depending on SDL or window APIs.
        std::thread::sleep(Duration::from_millis(16));
    }
}

#[inline]
fn scale(v: i32, factor: f32, max: u32) -> u32 {
    ((v.max(0) as f32 * factor) as u32).min(max - 1)
}
