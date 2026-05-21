//! Platform boundary for Rashamon Arc's browser shell.
//!
//! The browser core owns tabs, chrome state, persistence decisions, and renderer
//! commands. Platform backends provide display presentation, input, filesystem
//! roots, and coarse timer/event-loop hooks. The current shipping path is the
//! Linux desktop backend: SDL2/DRM for presentation, SDL2 for input, and XDG
//! paths for persistence. The Kamelot backend below is intentionally a compile-
//! time scaffold only; it documents the future OS boundary without replacing the
//! working Linux/WebKit path.

#![allow(dead_code)]

use rashamon_renderer::Framebuffer;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

pub trait DisplayBackend {
    fn present(&mut self, fb: &Framebuffer) -> io::Result<()>;
}

pub trait InputBackend {
    type Event;

    fn poll_event(&mut self) -> io::Result<Option<Self::Event>>;
}

pub trait FileSystemBackend {
    fn data_dir(&self) -> PathBuf;
    fn downloads_dir(&self) -> PathBuf;
}

pub trait NetworkBackend {
    fn name(&self) -> &'static str;
}

pub trait TimerBackend {
    fn sleep(&self, duration: Duration);
}

#[cfg(feature = "linux-desktop")]
pub struct LinuxDesktopBackend;

#[cfg(feature = "linux-desktop")]
impl LinuxDesktopBackend {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(feature = "linux-desktop")]
impl FileSystemBackend for LinuxDesktopBackend {
    fn data_dir(&self) -> PathBuf {
        linux_data_dir()
    }

    fn downloads_dir(&self) -> PathBuf {
        linux_downloads_dir()
    }
}

#[cfg(feature = "linux-desktop")]
impl NetworkBackend for LinuxDesktopBackend {
    fn name(&self) -> &'static str {
        "linux-desktop-network"
    }
}

#[cfg(feature = "linux-desktop")]
impl TimerBackend for LinuxDesktopBackend {
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[cfg(feature = "linux-desktop")]
pub fn default_data_dir() -> PathBuf {
    LinuxDesktopBackend::new().data_dir()
}

#[cfg(feature = "linux-desktop")]
pub fn linux_data_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("rashamon-arc")
}

#[cfg(feature = "linux-desktop")]
pub fn linux_downloads_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
        .join("RashamonArc")
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
pub struct KamelotBackend;

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
impl KamelotBackend {
    pub fn new() -> Self {
        Self
    }

    pub fn report_unimplemented(&self) {
        eprintln!("Platform: Kamelot scaffold (display/input/fs/network unimplemented)");
    }
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
impl FileSystemBackend for KamelotBackend {
    fn data_dir(&self) -> PathBuf {
        PathBuf::from("/kmfs/rashamon-arc")
    }

    fn downloads_dir(&self) -> PathBuf {
        PathBuf::from("/kmfs/rashamon-arc/downloads")
    }
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
impl NetworkBackend for KamelotBackend {
    fn name(&self) -> &'static str {
        "kamelot-syscall-network-stub"
    }
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
impl TimerBackend for KamelotBackend {
    fn sleep(&self, _duration: Duration) {
        // Future Kamelot syscall hook. No-op keeps the scaffold host-checkable.
    }
}

#[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
pub fn default_data_dir() -> PathBuf {
    KamelotBackend::new().data_dir()
}
