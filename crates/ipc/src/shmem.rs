//! Shared memory region for IPC

use memmap2::MmapMut;
use std::fs::OpenOptions;
use std::path::Path;

/// A shared memory region used for inter-process communication.
/// Supports both anonymous memory and file-backed memory maps.
pub struct SharedMemory {
    map: MmapMut,
    len: usize,
}

impl SharedMemory {
    /// Create an anonymous shared memory region of the given size.
    pub fn anonymous(size: usize) -> Result<Self, std::io::Error> {
        let file = tempfile_stub(size)?;
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(Self { map: mmap, len: size })
    }

    /// Create a file-backed shared memory region.
    pub fn from_file(path: &Path, size: usize) -> Result<Self, std::io::Error> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;
        file.set_len(size as u64)?;
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(Self { map: mmap, len: size })
    }

    /// Read a value from the shared memory at the given offset.
    pub fn read_slice(&self, offset: usize, len: usize) -> &[u8] {
        assert!(offset + len <= self.len);
        &self.map[offset..offset + len]
    }

    /// Write a value to the shared memory at the given offset.
    pub fn write_slice(&mut self, offset: usize, data: &[u8]) {
        assert!(offset + data.len() <= self.len);
        self.map[offset..offset + data.len()].copy_from_slice(data);
    }

    /// Mutable access to the entire region.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.map[..self.len]
    }

    /// Read-only access to the entire region.
    pub fn as_slice(&self) -> &[u8] {
        &self.map[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Stub to create a temporary file for anonymous mmap backing.
/// On Linux, uses /dev/shm for fast shared memory.
fn tempfile_stub(size: usize) -> Result<std::fs::File, std::io::Error> {
    let dir = if Path::new("/dev/shm").exists() {
        "/dev/shm"
    } else {
        "/tmp"
    };
    let (file, path) = tempfile::Builder::new()
        .prefix("rashamon-ipc-")
        .tempfile_in(dir)?
        .keep()?;
    file.set_len(size as u64)?;
    let _ = path; // suppress unused warning
    Ok(file)
}
