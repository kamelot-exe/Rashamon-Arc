//! Rashamon IPC — shared memory inter-process communication
//!
//! Provides low-latency, zero-copy IPC channels between browser processes
//! using memory-mapped shared memory regions.

mod channel;
mod protocol;
mod shmem;

pub use channel::{IpcChannel, IpcReceiver, IpcSender};
pub use protocol::*;
pub use shmem::SharedMemory;
