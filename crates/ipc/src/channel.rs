//! IPC channel abstraction over shared memory regions.

use crate::shmem::SharedMemory;
use bincode::Options;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

/// A typed sender for an IPC channel.
pub struct IpcSender<T> {
    shmem: Arc<Mutex<SharedMemory>>,
    header: Arc<AtomicU64>, // write offset + length
    _marker: std::marker::PhantomData<T>,
}

/// A typed receiver for an IPC channel.
pub struct IpcReceiver<T> {
    shmem: Arc<Mutex<SharedMemory>>,
    header: Arc<AtomicU64>,
    _marker: std::marker::PhantomData<T>,
}

/// A bidirectional IPC channel backed by shared memory.
pub struct IpcChannel<T> {
    pub sender: IpcSender<T>,
    pub receiver: IpcReceiver<T>,
}

impl<T> IpcSender<T>
where
    T: Serialize,
{
    pub fn send(&self, msg: &T) -> Result<(), Box<dyn std::error::Error>> {
        let data = bincode::DefaultOptions::new().serialize(msg)?;
        let mut shmem = self.shmem.lock().unwrap();
        let len = data.len();
        // Simple protocol: write length (4 bytes) + data
        if len > shmem.len() - 8 {
            return Err("Message too large for shared memory buffer".into());
        }
        shmem.write_slice(0, &(len as u32).to_le_bytes());
        shmem.write_slice(4, &data);
        self.header.store(len as u64, Ordering::Release);
        Ok(())
    }
}

impl<T> IpcReceiver<T>
where
    T: for<'de> Deserialize<'de>,
{
    pub fn try_recv(&self) -> Option<Result<T, Box<dyn std::error::Error>>> {
        let shmem = self.shmem.lock().unwrap();
        let len = self.header.load(Ordering::Acquire);
        if len == 0 {
            return None;
        }
        let slice = shmem.read_slice(4, len as usize);
        let result = bincode::DefaultOptions::new().deserialize(slice).map_err(|e| e.into());
        Some(result)
    }
}

impl<T> IpcChannel<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    /// Create a new IPC channel pair (sender, receiver) sharing the same memory.
    pub fn new(size: usize) -> Result<(IpcSender<T>, IpcReceiver<T>), Box<dyn std::error::Error>> {
        let shmem = SharedMemory::anonymous(size)?;
        let shmem = Arc::new(Mutex::new(shmem));
        let header = Arc::new(AtomicU64::new(0));
        let sender = IpcSender {
            shmem: shmem.clone(),
            header: header.clone(),
            _marker: std::marker::PhantomData,
        };
        let receiver = IpcReceiver {
            shmem,
            header,
            _marker: std::marker::PhantomData,
        };
        Ok((sender, receiver))
    }
}
