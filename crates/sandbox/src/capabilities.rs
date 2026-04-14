//! Capability-based access control.

/// A capability represents a specific permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Access to network (network process only)
    NetworkAccess,
    /// Read from clipboard (requires user gesture)
    ClipboardRead,
    /// Write to clipboard (requires user gesture)
    ClipboardWrite,
    /// Show file picker (returns capability handles only)
    FilePicker,
    /// Access to a specific file handle
    FileRead,
    FileWrite,
    /// Create child processes
    SpawnProcess,
    /// Access to raw devices (framebuffer, input)
    DeviceAccess,
}

/// A set of capabilities for a process.
#[derive(Debug, Clone)]
pub struct CapabilitySet {
    granted: Vec<Capability>,
}

impl CapabilitySet {
    pub fn empty() -> Self {
        Self { granted: vec![] }
    }

    pub fn ui_process() -> Self {
        Self {
            granted: vec![
                Capability::DeviceAccess,
                Capability::ClipboardRead,
                Capability::ClipboardWrite,
                Capability::FilePicker,
                Capability::NetworkAccess,
            ],
        }
    }

    pub fn network_process() -> Self {
        Self {
            granted: vec![Capability::NetworkAccess],
        }
    }

    pub fn renderer_process() -> Self {
        // Renderer has NO direct capabilities — must IPC for everything.
        Self { granted: vec![] }
    }

    pub fn add(&mut self, cap: Capability) {
        if !self.granted.contains(&cap) {
            self.granted.push(cap);
        }
    }

    pub fn has(&self, cap: Capability) -> bool {
        self.granted.contains(&cap)
    }

    pub fn revoke(&mut self, cap: Capability) {
        self.granted.retain(|c| *c != cap);
    }
}
