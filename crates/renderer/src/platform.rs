//! Renderer-side platform seams.
//!
//! WebKitGTK remains the Linux desktop renderer for v0.5. Kamelot support is a
//! scaffold only: future renderer code should route OS paths, timers, and
//! network/syscall integration through this module instead of embedding Linux
//! assumptions in browser logic.

#![allow(dead_code)]

use std::path::PathBuf;

pub trait RendererFileSystem {
    fn data_dir(&self) -> PathBuf;
    fn downloads_dir(&self) -> PathBuf;
}

pub trait RendererNetwork {
    fn name(&self) -> &'static str;
}

#[cfg(feature = "webkit")]
pub struct LinuxDesktopPlatform;

#[cfg(feature = "webkit")]
impl RendererFileSystem for LinuxDesktopPlatform {
    fn data_dir(&self) -> PathBuf {
        linux_data_dir()
    }

    fn downloads_dir(&self) -> PathBuf {
        linux_downloads_dir()
    }
}

#[cfg(feature = "webkit")]
impl RendererNetwork for LinuxDesktopPlatform {
    fn name(&self) -> &'static str {
        "webkitgtk-network"
    }
}

#[cfg(feature = "webkit")]
pub fn default_data_dir() -> PathBuf {
    LinuxDesktopPlatform.data_dir()
}

#[cfg(feature = "webkit")]
pub fn default_downloads_dir() -> PathBuf {
    LinuxDesktopPlatform.downloads_dir()
}

#[cfg(feature = "webkit")]
fn linux_data_dir() -> PathBuf {
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

#[cfg(feature = "webkit")]
fn linux_downloads_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
        .join("RashamonArc")
}

#[cfg(all(feature = "kamelot", not(feature = "webkit")))]
pub struct KamelotPlatform;

#[cfg(all(feature = "kamelot", not(feature = "webkit")))]
impl RendererFileSystem for KamelotPlatform {
    fn data_dir(&self) -> PathBuf {
        PathBuf::from("/kmfs/rashamon-arc")
    }

    fn downloads_dir(&self) -> PathBuf {
        PathBuf::from("/kmfs/rashamon-arc/downloads")
    }
}

#[cfg(all(feature = "kamelot", not(feature = "webkit")))]
impl RendererNetwork for KamelotPlatform {
    fn name(&self) -> &'static str {
        "kamelot-network-syscall-stub"
    }
}
