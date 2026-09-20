use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub id: Uuid,
    pub body: Message,
}

impl Envelope {
    pub fn new(body: Message) -> Self {
        Self {
            id: Uuid::new_v4(),
            body,
        }
    }

    pub fn reply(request_id: Uuid, body: Message) -> Self {
        Self {
            id: request_id,
            body,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    Handshake(Handshake),
    HandshakeAck(HandshakeAck),
    Heartbeat(Heartbeat),
    HeartbeatAck,
    Screenshot(ScreenshotReq),
    ScreenshotReply(ScreenshotReply),
    MouseMove(MouseMoveReq),
    MouseClick(MouseClickReq),
    KeyType(KeyTypeReq),
    KeyPress(KeyPressReq),
    Scroll(ScrollReq),
    CommandAck(CommandAck),
    BrowserStateReport(BrowserStateReport),
    RouteStateReport(RouteStateReport),
    BrowserStart,
    BrowserStop,
    SecretTypeRequest(SecretTypeRequest),
    SecretTypeResponse(SecretTypeResponse),
    SecretRefused(SecretRefused),
    Shutdown,
    ShutdownAck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handshake {
    pub version: u32,
    pub hostname: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeAck {
    pub version: u32,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotReq {
    pub format: ImageFormat,
    #[serde(default)]
    pub frame_id: Option<Uuid>,
    #[serde(default)]
    pub max_edge: Option<u32>,
    #[serde(default)]
    pub region: Option<Region>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ImageFormat {
    Png,
    Jpeg { quality: u8 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotReply {
    pub frame_id: Uuid,
    pub native_width: u32,
    pub native_height: u32,
    pub returned_width: u32,
    pub returned_height: u32,
    pub scale: f64,
    pub format: ImageFormat,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorPosition {
    pub x: i32,
    pub y: i32,
    pub frame_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MouseMoveReq {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MouseClickReq {
    pub x: i32,
    pub y: i32,
    pub button: MouseButton,
    pub click_type: ClickType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClickType {
    Single,
    Double,
    Triple,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyTypeReq {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPressReq {
    pub key: String,
    pub modifiers: Vec<KeyModifier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KeyModifier {
    Ctrl,
    Alt,
    Shift,
    Super,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrollReq {
    pub x: i32,
    pub y: i32,
    pub delta_x: i32,
    pub delta_y: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandAck {
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyProtocol {
    Socks5,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Route {
    Direct,
    Proxy {
        address: String,
        port: u16,
        auth: Option<ProxyAuth>,
        protocol: ProxyProtocol,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyAuth {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteState {
    Up,
    Down { reason: String },
    Checking,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteStateReport {
    pub state: RouteState,
    pub exit_ip: Option<String>,
    pub asn: Option<u32>,
    pub country: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserState {
    NotInstalled,
    Installing,
    Installed,
    Starting,
    Running,
    Crashed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserStateReport {
    pub state: BrowserState,
    pub pid: Option<u32>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    pub accept_languages: String,
    pub download_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretTypeRequest {
    pub name: String,
    pub observed_domain: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretTypeResponse {
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretRefused {
    pub reason: String,
    pub observed_domain: Option<String>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            accept_languages: "en-US,en".into(),
            download_dir: "/home/taboom/Downloads".into(),
        }
    }
}
