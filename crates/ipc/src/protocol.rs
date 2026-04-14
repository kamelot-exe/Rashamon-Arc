//! IPC protocol definitions.
//!
//! All messages exchanged between browser processes use these types.

use serde::{Deserialize, Serialize};

/// Top-level IPC message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcMessage {
    // UI <-> Renderer
    Navigate { url: String },
    NavigateResult { success: bool, error: Option<String> },
    RenderUpdate { dirty_rect: Option<Rect>, frame_id: u64 },
    InputEvent { event: InputEvent },
    JsEval { source: String, callback_id: u64 },
    JsResult { callback_id: u64, result: String },
    TitleChanged { title: String },
    LoadingStateChanged { loading: bool },

    // UI <-> Network
    FetchRequest { request: NetworkRequest },
    FetchResponse { response: NetworkResponse },
    CookieStore { cookies: Vec<Cookie> },
    AdblockStats { blocked: u64, total: u64 },
    AdblockToggle { enabled: bool, rule: String },

    // Control
    Ping,
    Pong,
    Shutdown,
}

/// A rectangle for dirty-region repaints.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Input events forwarded from UI to renderer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove { x: i32, y: i32 },
    MouseDown { x: i32, y: i32, button: u8 },
    MouseUp { x: i32, y: i32, button: u8 },
    KeyPress { key_code: u32, modifiers: u8 },
    Scroll { delta: i32 },
}

/// A network request from renderer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub origin: String,
}

/// A network response from the network process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub blocked: bool,
    pub block_reason: Option<String>,
}

/// A cookie entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires: Option<i64>,
}
