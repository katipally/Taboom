use crate::handoff::{HandoffManager, HandoffStatus};
use crate::lease::LeaseManager;
use crate::local::{finish_video, png_size, LocalExecutor, Rect, Screen};
use crate::persona::{self, PersonaRegistry};
use crate::recording::{self, Recorder};
use crate::tools::{ToolCall, ToolRegion, translate_computer_tool};
use anyhow::{bail, Context};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use taboom_proto::Message;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub is_error: bool,
    /// Sent to the model as MCP image blocks, ahead of `content`.
    #[serde(skip)]
    pub images: Vec<Image>,
}

#[derive(Debug, Clone)]
pub struct Image {
    pub base64: String,
    pub mime: &'static str,
}

impl ToolResult {
    pub fn ok(content: Value) -> Self {
        Self {
            content,
            error: None,
            is_error: false,
            images: vec![],
        }
    }

    pub fn err(message: &str) -> Self {
        Self {
            content: json!({ "error": message }),
            error: Some(message.to_string()),
            is_error: true,
            images: vec![],
        }
    }

    fn with_image(mut self, bytes: &[u8], mime: &'static str) -> Self {
        self.images.push(Image {
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            mime,
        });
        self
    }
}

type Res = anyhow::Result<ToolResult>;

pub type ChannelSender = tokio::sync::mpsc::Sender<Message>;

/// Longest edge of screenshots by default. Larger images get downscaled by the model's API,
/// which silently skews the coordinates it sends back.
const DEFAULT_MAX_EDGE: u32 = 1280;
const PASTE_THRESHOLD_CHARS: usize = 400;
const FILE_GET_LIMIT: u64 = 20 * 1024 * 1024;
const RECORD_FRAME_DELAY: Duration = Duration::from_millis(400);

/// The pixel space of the latest full screenshot a client saw.
#[derive(Debug, Clone, Copy)]
struct View {
    id: Uuid,
    sx: f64,
    sy: f64,
    screen: Screen,
}

impl View {
    fn fit(screen: Screen, max_edge: u32) -> f64 {
        (max_edge as f64 / screen.width.max(screen.height) as f64).min(1.0)
    }
}

pub struct ToolHandler {
    lease_mgr: Mutex<LeaseManager>,
    handoff_mgr: Mutex<HandoffManager>,
    personas: Arc<PersonaRegistry>,
    channel: Mutex<Option<ChannelSender>>,
    local: Option<LocalExecutor>,
    recorder: Arc<Recorder>,
    views: Mutex<HashMap<String, View>>,
    videos: Mutex<HashMap<String, (std::process::Child, PathBuf)>>,
    files_dir: PathBuf,
    max_edge: u32,
}

impl ToolHandler {
    pub fn new(
        lease_mgr: LeaseManager,
        handoff_mgr: HandoffManager,
        personas: Arc<PersonaRegistry>,
        recorder: Arc<Recorder>,
    ) -> Self {
        let local = if std::env::var("TABOOM_LOCAL").is_ok() {
            let local = LocalExecutor::new();
            local.start_idle();
            Some(local)
        } else {
            None
        };
        let files_dir = std::env::var_os("TABOOM_FILES_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads")))
            .unwrap_or_else(|| PathBuf::from("Downloads"));
        let max_edge = std::env::var("TABOOM_MAX_EDGE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_EDGE);
        Self {
            lease_mgr: Mutex::new(lease_mgr),
            handoff_mgr: Mutex::new(handoff_mgr),
            personas,
            channel: Mutex::new(None),
            local,
            recorder,
            views: Mutex::new(HashMap::new()),
            videos: Mutex::new(HashMap::new()),
            files_dir,
            max_edge,
        }
    }

    pub fn recorder(&self) -> &Recorder {
        &self.recorder
    }

    pub fn set_channel(&self, tx: ChannelSender) {
        *self.channel.lock().unwrap() = Some(tx);
    }

    fn try_dispatch(&self, msg: &Message) {
        if let Some(tx) = self.channel.lock().unwrap().as_ref() {
            let _ = tx.try_send(msg.clone());
        }
    }

    /// Runs one tool call and records it in the caller's session.
    pub fn handle(&self, call: ToolCall, client_id: &str) -> ToolResult {
        let call = match &call {
            ToolCall::Computer { .. } => match translate_computer_tool(&call) {
                Some(native) => native,
                None => return ToolResult::err("unsupported computer action"),
            },
            _ => call,
        };

        let started = Instant::now();
        let result = self.dispatch(&call, client_id);

        if let (ToolCall::PersonaAcquire { id }, false) = (&call, result.is_error) {
            match self.recorder.start(client_id, id) {
                Ok(rec) => self.start_video(client_id, &rec),
                Err(e) => tracing::warn!("recording not started: {e}"),
            }
            if let Some(local) = &self.local {
                local.set_persona(id);
                local.set_idle(true);
            }
        }

        let frame = match (&call, &self.local, result.is_error) {
            (ToolCall::Screenshot { .. } | ToolCall::Zoom { .. }, _, false) => result
                .images
                .first()
                .and_then(|i| base64::engine::general_purpose::STANDARD.decode(&i.base64).ok())
                .map_or(recording::Frame::None, |b| recording::Frame::Bytes(b, "png")),
            (
                ToolCall::Click { .. } | ToolCall::Move { .. } | ToolCall::Drag { .. }
                | ToolCall::Scroll { .. } | ToolCall::Type { .. } | ToolCall::Key { .. }
                | ToolCall::OpenUrl { .. } | ToolCall::Wait { .. } | ToolCall::MouseDown { .. }
                | ToolCall::MouseUp { .. } | ToolCall::KeyDown { .. } | ToolCall::KeyUp { .. }
                | ToolCall::HoldKey { .. },
                Some(_),
                _,
            ) => recording::Frame::Pending("png"),
            _ => recording::Frame::None,
        };
        let pending = self.recorder.record(
            client_id,
            &tool_name(&call),
            record_args(&call),
            result.error.as_deref(),
            started.elapsed().as_millis() as u64,
            frame,
        );
        if let (Some(path), Some(local)) = (pending, self.local.clone()) {
            std::thread::spawn(move || {
                std::thread::sleep(RECORD_FRAME_DELAY);
                match local.capture(None, 0.5, "png") {
                    Ok(jpg) => {
                        let _ = std::fs::write(path, jpg);
                    }
                    Err(e) => tracing::debug!("recording frame skipped: {e}"),
                }
            });
        }

        if let (ToolCall::PersonaRelease, false) = (&call, result.is_error) {
            if let Some(local) = &self.local {
                local.set_idle(false);
                let _ = local.release_all();
            }
            self.stop_video(client_id);
            self.recorder.stop(client_id);
            self.views.lock().unwrap().remove(client_id);
        }
        result
    }

    /// Screen video for the session, next to its events. Local mode only.
    fn start_video(&self, client_id: &str, rec: &str) {
        let (Some(local), Ok(dir)) = (&self.local, self.recorder.session_path(rec)) else {
            return;
        };
        let mut videos = self.videos.lock().unwrap();
        if videos.contains_key(client_id) {
            return;
        }
        let mkv = dir.join("video.mkv");
        match local.start_video(&mkv) {
            Ok(child) => {
                videos.insert(client_id.to_string(), (child, mkv));
            }
            Err(e) => tracing::warn!("video not started: {e:#}"),
        }
    }

    /// Finalizing takes a few seconds, so it runs off the caller's path.
    fn stop_video(&self, client_id: &str) {
        if let Some((child, mkv)) = self.videos.lock().unwrap().remove(client_id) {
            std::thread::spawn(move || {
                if let Err(e) = finish_video(child, &mkv) {
                    tracing::warn!("video not finalized: {e:#}");
                }
            });
        }
    }

    fn dispatch(&self, call: &ToolCall, client_id: &str) -> ToolResult {
        match call {
            ToolCall::PersonaList => self.handle_persona_list(),
            ToolCall::PersonaAcquire { id } => self.handle_persona_acquire(id, client_id),
            ToolCall::PersonaRelease => self.handle_persona_release(client_id),
            ToolCall::PersonaStatus => self.handle_persona_status(client_id),
            ToolCall::PersonaCreate { name, timezone, locale, keyboard_layout, languages, accept_languages } => {
                settle(self.persona_create(name, timezone, locale, keyboard_layout, languages, accept_languages))
            }

            ToolCall::HandoffStart { reason } => self.handle_handoff_start(client_id, reason),
            ToolCall::HandoffWait { id, timeout_s } => self.handle_handoff_wait(id, *timeout_s),
            ToolCall::HandoffResolve { id, action } => self.handle_handoff_resolve(id, action),

            ToolCall::RecordingList { persona, limit } => {
                self.recording_list(persona.as_deref(), limit.unwrap_or(20).clamp(1, 500), client_id)
            }
            ToolCall::RecordingGet { id, step, from_step, limit, frames } => settle(self.recording_get(
                client_id,
                id.as_deref(),
                *step,
                from_step.unwrap_or(1),
                limit.unwrap_or(50).clamp(1, 500),
                *frames,
            )),
            ToolCall::RecordingShare { id, expires_in_s } => settle((|| {
                let id = self.recording_id(client_id, id.as_deref())?;
                let ttl = expires_in_s.unwrap_or(7 * 24 * 3600).clamp(60, 90 * 24 * 3600);
                let (url, video_url) = self.recorder.share_url(&id, ttl)?;
                Ok(ToolResult::ok(json!({ "id": id, "url": url, "video_url": video_url, "expires_in_s": ttl })))
            })()),

            ToolCall::ViewUrl => self.require_lease_then(client_id, || {
                ToolResult::ok(json!({
                    "url": crate::liveview::watch_url(),
                    "takeover_url": crate::liveview::takeover_url(),
                }))
            }),

            ToolCall::Computer { .. } => ToolResult::err("unsupported computer action"),

            device_call => self.require_lease_then(client_id, || match &self.local {
                Some(local) => settle(match hand_use(device_call) {
                    Some(on_keyboard) => local.exclusive(Some(on_keyboard), || self.local_call(local, device_call, client_id)),
                    None => self.local_call(local, device_call, client_id),
                }),
                None => self.guest_call(device_call),
            }),
        }
    }

    /// Screen, input, files and clipboard against the container's own desktop.
    fn local_call(&self, local: &LocalExecutor, call: &ToolCall, client_id: &str) -> Res {
        match call {
            ToolCall::Screenshot { max_edge, region, .. } => {
                let screen = local.screen()?;
                let max_edge = max_edge.unwrap_or(self.max_edge).clamp(200, 4096);
                if let Some(region) = region {
                    return self.zoom(local, client_id, region, max_edge);
                }
                let png = local.capture(None, View::fit(screen, max_edge), "png")?;
                let (w, h) = png_size(&png).context("grim returned a non-PNG image")?;
                let view = View {
                    id: Uuid::new_v4(),
                    sx: w as f64 / screen.width as f64,
                    sy: h as f64 / screen.height as f64,
                    screen,
                };
                self.views.lock().unwrap().insert(client_id.to_string(), view);
                Ok(ToolResult::ok(json!({
                    "frame_id": view.id.to_string(),
                    "width": w,
                    "height": h,
                    "screen_width": screen.width,
                    "screen_height": screen.height,
                    "note": "coordinates you send are in this image's pixel space",
                }))
                .with_image(&png, "image/png"))
            }
            ToolCall::Zoom { region } => self.zoom(local, client_id, region, 4096),
            ToolCall::CursorPosition => {
                let view = self.view(local, client_id)?;
                let (x, y) = local.cursor()?;
                Ok(ToolResult::ok(json!({
                    "x": (x as f64 * view.sx).round() as i64,
                    "y": (y as f64 * view.sy).round() as i64,
                    "frame_id": view.id.to_string(),
                })))
            }
            ToolCall::Click { x, y, button, count, describe, frame_id } => {
                if let Some(fid) = frame_id {
                    let current = self.views.lock().unwrap().get(client_id).map(|v| v.id.to_string());
                    if current.as_deref().is_some_and(|c| c != fid) {
                        bail!("frame {fid} is stale; take a new screenshot and use its coordinates");
                    }
                }
                let (nx, ny) = self.to_native(local, client_id, *x, *y)?;
                local.mouse_click(nx, ny, button, (*count).clamp(1, 3))?;
                Ok(ToolResult::ok(json!({
                    "clicked": true, "x": x, "y": y, "button": button, "count": count, "describe": describe,
                })))
            }
            ToolCall::Move { x, y, .. } => {
                let (nx, ny) = self.to_native(local, client_id, *x, *y)?;
                local.mouse_move(nx, ny)?;
                Ok(ToolResult::ok(json!({ "moved": true, "x": x, "y": y })))
            }
            ToolCall::Drag { from, to, describe } => {
                let a = self.to_native(local, client_id, from.x, from.y)?;
                let b = self.to_native(local, client_id, to.x, to.y)?;
                local.drag(a, b)?;
                Ok(ToolResult::ok(json!({
                    "dragged": true,
                    "from": { "x": from.x, "y": from.y },
                    "to": { "x": to.x, "y": to.y },
                    "describe": describe,
                })))
            }
            ToolCall::Scroll { x, y, direction, amount } => {
                let (nx, ny) = match (x, y) {
                    (Some(x), Some(y)) => self.to_native(local, client_id, *x, *y)?,
                    _ => {
                        let s = local.screen()?;
                        (s.width / 2, s.height / 2)
                    }
                };
                let n = (*amount).clamp(1, 50);
                let (dx, dy) = match direction.as_str() {
                    "up" => (0, -n),
                    "down" => (0, n),
                    "left" => (-n, 0),
                    "right" => (n, 0),
                    other => bail!("direction must be up, down, left or right, not {other:?}"),
                };
                local.scroll(nx, ny, dx, dy)?;
                Ok(ToolResult::ok(json!({ "scrolled": true, "direction": direction, "amount": n })))
            }
            ToolCall::Type { text, mode, submit } => {
                if text.contains("<secret>") {
                    bail!("<secret> placeholders need the vault, which Docker mode does not have yet");
                }
                let typeable = LocalExecutor::can_type(text);
                let paste = match mode.as_str() {
                    "paste" => true,
                    "keys" if !typeable => bail!("text has characters with no key on this layout; use mode paste or auto"),
                    "keys" => false,
                    _ => !typeable || text.chars().count() > PASTE_THRESHOLD_CHARS,
                };
                if paste {
                    local.clipboard_set(text)?;
                    local.key_press("ctrl+v")?;
                } else if !text.is_empty() {
                    local.key_type(text)?;
                }
                if *submit {
                    local.key_press("Return")?;
                }
                Ok(ToolResult::ok(json!({
                    "typed": true,
                    "chars": text.chars().count(),
                    "mode": if paste { "paste" } else { "keys" },
                    "submitted": submit,
                })))
            }
            ToolCall::Key { combo } => {
                if combo.trim().is_empty() {
                    bail!("combo is empty");
                }
                local.key_press(combo)?;
                Ok(ToolResult::ok(json!({ "pressed": true, "combo": combo })))
            }
            ToolCall::MouseDown { button, x, y } => {
                if let (Some(x), Some(y)) = (x, y) {
                    let (nx, ny) = self.to_native(local, client_id, *x, *y)?;
                    local.mouse_move(nx, ny)?;
                }
                local.mouse_button(button, true)?;
                Ok(ToolResult::ok(json!({ "held": button })))
            }
            ToolCall::MouseUp { button } => {
                local.mouse_button(button, false)?;
                Ok(ToolResult::ok(json!({ "released": button })))
            }
            ToolCall::KeyDown { key } => {
                local.key_state(key, true)?;
                Ok(ToolResult::ok(json!({ "held": key })))
            }
            ToolCall::KeyUp { key } => {
                local.key_state(key, false)?;
                Ok(ToolResult::ok(json!({ "released": key })))
            }
            ToolCall::HoldKey { key, duration_ms } => {
                let ms = duration_ms.unwrap_or(1000).min(10_000);
                local.key_state(key, true)?;
                std::thread::sleep(Duration::from_millis(ms));
                local.key_state(key, false)?;
                Ok(ToolResult::ok(json!({ "held": key, "ms": ms })))
            }
            ToolCall::Wait { ms, until_settled } => {
                let started = Instant::now();
                if until_settled.unwrap_or(false) {
                    let settled = wait_settled(local, Duration::from_millis(ms.unwrap_or(3000).min(60_000)))?;
                    return Ok(ToolResult::ok(json!({
                        "settled": settled,
                        "status": if settled { "settled" } else { "still_changing" },
                        "waited_ms": started.elapsed().as_millis() as u64,
                    })));
                }
                std::thread::sleep(Duration::from_millis(ms.unwrap_or(1000).min(60_000)));
                Ok(ToolResult::ok(json!({ "waited_ms": started.elapsed().as_millis() as u64 })))
            }
            ToolCall::OpenUrl { url } => {
                if url.trim().is_empty() {
                    bail!("url is empty");
                }
                local.open_url(url)?;
                Ok(ToolResult::ok(json!({ "opened": true, "url": url })))
            }
            ToolCall::FilesPut { name, data_base64 } => {
                let name = safe_file_name(name)?;
                let data = base64::engine::general_purpose::STANDARD
                    .decode(data_base64.trim())
                    .context("data_base64 is not valid base64")?;
                std::fs::create_dir_all(&self.files_dir)?;
                let path = self.files_dir.join(name);
                std::fs::write(&path, &data)?;
                Ok(ToolResult::ok(json!({ "path": path, "bytes": data.len() })))
            }
            ToolCall::FilesGet { path } => {
                let path = self.resolve_file(path)?;
                let len = std::fs::metadata(&path)?.len();
                if len > FILE_GET_LIMIT {
                    bail!("{} is {len} bytes; the limit is {FILE_GET_LIMIT}", path.display());
                }
                let data = std::fs::read(&path)?;
                Ok(ToolResult::ok(json!({
                    "path": path,
                    "bytes": data.len(),
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(&data),
                })))
            }
            ToolCall::FilesList => {
                let mut files: Vec<(Value, std::time::SystemTime)> = match std::fs::read_dir(&self.files_dir) {
                    Ok(entries) => entries
                        .flatten()
                        .filter_map(|e| {
                            let meta = e.metadata().ok()?;
                            let modified = meta.modified().ok()?;
                            meta.is_file().then(|| {
                                let ts: chrono::DateTime<chrono::Utc> = modified.into();
                                (json!({
                                    "name": e.file_name().to_string_lossy(),
                                    "bytes": meta.len(),
                                    "modified": ts.to_rfc3339(),
                                }), modified)
                            })
                        })
                        .collect(),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
                    Err(e) => return Err(e.into()),
                };
                files.sort_by(|a, b| b.1.cmp(&a.1));
                let files: Vec<Value> = files.into_iter().map(|f| f.0).collect();
                Ok(ToolResult::ok(json!({ "dir": self.files_dir, "files": files })))
            }
            ToolCall::ClipboardGet => Ok(ToolResult::ok(json!({ "text": local.clipboard_get()? }))),
            ToolCall::ClipboardSet { text } => {
                local.clipboard_set(text)?;
                Ok(ToolResult::ok(json!({ "set": true, "chars": text.chars().count() })))
            }
            other => bail!("{} is not a device tool", tool_name(other)),
        }
    }

    fn zoom(&self, local: &LocalExecutor, client_id: &str, region: &ToolRegion, max_edge: u32) -> Res {
        let view = self.view(local, client_id)?;
        let s = view.screen;
        let x = ((region.x as f64 / view.sx) as u32).min(s.width - 1);
        let y = ((region.y as f64 / view.sy) as u32).min(s.height - 1);
        let w = ((region.w as f64 / view.sx).round() as u32).clamp(1, s.width - x);
        let h = ((region.h as f64 / view.sy).round() as u32).clamp(1, s.height - y);
        let rect = Rect { x, y, w, h };
        let png = local.capture(Some(rect), (max_edge as f64 / w.max(h) as f64).min(1.0), "png")?;
        let (iw, ih) = png_size(&png).context("grim returned a non-PNG image")?;
        Ok(ToolResult::ok(json!({
            "width": iw,
            "height": ih,
            "screen_region": { "x": x, "y": y, "w": w, "h": h },
            "note": "inspection only; keep using the latest screenshot's coordinates",
        }))
        .with_image(&png, "image/png"))
    }

    /// The client's latest view, or the default one if they have not taken a screenshot yet.
    fn view(&self, local: &LocalExecutor, client_id: &str) -> anyhow::Result<View> {
        let screen = local.screen()?;
        let view = self.views.lock().unwrap().get(client_id).copied();
        match view {
            Some(v) if v.screen != screen => bail!(
                "the screen changed size ({}x{} -> {}x{}); take a new screenshot",
                v.screen.width, v.screen.height, screen.width, screen.height
            ),
            Some(v) => Ok(v),
            None => {
                let s = View::fit(screen, self.max_edge);
                Ok(View { id: Uuid::nil(), sx: s, sy: s, screen })
            }
        }
    }

    fn to_native(&self, local: &LocalExecutor, client_id: &str, x: i32, y: i32) -> anyhow::Result<(u32, u32)> {
        let v = self.view(local, client_id)?;
        let (w, h) = (
            (v.screen.width as f64 * v.sx).round() as i32,
            (v.screen.height as f64 * v.sy).round() as i32,
        );
        if x < 0 || y < 0 || x >= w || y >= h {
            bail!("({x}, {y}) is outside the screenshot ({w}x{h})");
        }
        let nx = ((x as f64 + 0.5) / v.sx) as u32;
        let ny = ((y as f64 + 0.5) / v.sy) as u32;
        Ok((nx.min(v.screen.width - 1), ny.min(v.screen.height - 1)))
    }

    fn resolve_file(&self, path: &str) -> anyhow::Result<PathBuf> {
        let root = self.files_dir.canonicalize().context("files folder does not exist yet")?;
        let candidate = if Path::new(path).is_absolute() { PathBuf::from(path) } else { root.join(path) };
        let resolved = candidate.canonicalize().with_context(|| format!("{path} not found"))?;
        if !resolved.starts_with(&root) || !resolved.is_file() {
            bail!("{path} is not a file in {}", root.display());
        }
        Ok(resolved)
    }

    fn recording_id(&self, client_id: &str, id: Option<&str>) -> anyhow::Result<String> {
        match id {
            Some(id) => Ok(id.to_string()),
            None => self
                .recorder
                .current(client_id)
                .context("no active recording; pass an id from recording_list"),
        }
    }

    fn recording_list(&self, persona: Option<&str>, limit: usize, client_id: &str) -> ToolResult {
        let current = self.recorder.current(client_id);
        let sessions: Vec<Value> = self
            .recorder
            .list(persona, limit)
            .into_iter()
            .map(|(m, steps)| json!({
                "id": m.id,
                "persona": m.persona,
                "client": m.client,
                "started_at": m.started_at.to_rfc3339(),
                "ended_at": m.ended_at.map(|t| t.to_rfc3339()),
                "steps": steps,
                "video": self.recorder.video(&m.id),
                "current": current.as_deref() == Some(m.id.as_str()),
            }))
            .collect();
        ToolResult::ok(json!({ "recordings": sessions }))
    }

    fn recording_get(
        &self,
        client_id: &str,
        id: Option<&str>,
        step: Option<u32>,
        from_step: u32,
        limit: usize,
        frames: bool,
    ) -> Res {
        const MAX_FRAMES: usize = 10;
        let id = self.recording_id(client_id, id)?;
        let meta = self.recorder.meta(&id)?;
        let (events, total) = match step {
            Some(s) => self.recorder.events(&id, s, 1)?,
            None => self.recorder.events(&id, from_step, limit)?,
        };
        if step.is_some_and(|s| events.first().map(|e| e.step) != Some(s)) {
            bail!("recording {id} has no step {}", step.unwrap_or_default());
        }
        let mut result = ToolResult::ok(json!({
            "id": id,
            "persona": meta.persona,
            "started_at": meta.started_at.to_rfc3339(),
            "ended_at": meta.ended_at.map(|t| t.to_rfc3339()),
            "total_steps": total,
            "video": self.recorder.video(&id),
            "next_from_step": events.last().filter(|_| step.is_none() && events.len() == limit).map(|e| e.step + 1),
            "steps": events,
        }));
        let want = if step.is_some() { 1 } else if frames { MAX_FRAMES } else { 0 };
        for e in events.iter().filter(|e| e.frame.is_some()).take(want) {
            let name = e.frame.as_deref().unwrap_or_default();
            if let Ok(bytes) = std::fs::read(self.recorder.frame_path(&id, name)?) {
                let mime = if name.ends_with(".png") { "image/png" } else { "image/jpeg" };
                result = result.with_image(&bytes, mime);
            }
        }
        Ok(result)
    }

    /// VM mode: forward to the guest over the control channel.
    fn guest_call(&self, call: &ToolCall) -> ToolResult {
        use taboom_proto::{ClickType, ImageFormat, MouseButton, MouseClickReq, ScreenshotReq};
        let msg = match call {
            ToolCall::Screenshot { frame_id, max_edge, region } => Message::Screenshot(ScreenshotReq {
                format: ImageFormat::Png,
                frame_id: frame_id.as_deref().and_then(|s| s.parse().ok()),
                max_edge: *max_edge,
                region: region.as_ref().map(|r| taboom_proto::Region { x: r.x, y: r.y, width: r.w, height: r.h }),
            }),
            ToolCall::Zoom { region } => Message::Screenshot(ScreenshotReq {
                format: ImageFormat::Png,
                frame_id: None,
                max_edge: None,
                region: Some(taboom_proto::Region { x: region.x, y: region.y, width: region.w, height: region.h }),
            }),
            ToolCall::Click { x, y, button, count, .. } => Message::MouseClick(MouseClickReq {
                x: *x,
                y: *y,
                button: match button.as_str() {
                    "right" => MouseButton::Right,
                    "middle" => MouseButton::Middle,
                    _ => MouseButton::Left,
                },
                click_type: match count {
                    2 => ClickType::Double,
                    3 => ClickType::Triple,
                    _ => ClickType::Single,
                },
            }),
            ToolCall::Move { x, y, .. } => Message::MouseMove(taboom_proto::MouseMoveReq { x: *x, y: *y }),
            ToolCall::Drag { from, to, .. } => {
                self.try_dispatch(&Message::MouseMove(taboom_proto::MouseMoveReq { x: from.x, y: from.y }));
                Message::MouseMove(taboom_proto::MouseMoveReq { x: to.x, y: to.y })
            }
            ToolCall::Scroll { x, y, direction, amount } => {
                let n = *amount;
                let (delta_x, delta_y) = match direction.as_str() {
                    "up" => (0, -n),
                    "down" => (0, n),
                    "left" => (-n, 0),
                    _ => (n, 0),
                };
                Message::Scroll(taboom_proto::ScrollReq { x: x.unwrap_or(0), y: y.unwrap_or(0), delta_x, delta_y })
            }
            ToolCall::Type { text, .. } => Message::KeyType(taboom_proto::KeyTypeReq { text: text.clone() }),
            ToolCall::Key { combo } => Message::KeyPress(taboom_proto::KeyPressReq { key: combo.clone(), modifiers: vec![] }),
            other => return ToolResult::err(&format!("{} is not available in VM mode yet", tool_name(other))),
        };
        self.try_dispatch(&msg);
        ToolResult::ok(json!({ "dispatched": tool_name(call), "message": format!("{msg:?}") }))
    }

    fn handle_persona_status(&self, client_id: &str) -> ToolResult {
        let mgr = self.lease_mgr.lock().unwrap();
        let Some(lease) = mgr.lease_for(client_id) else {
            return ToolResult::err("no active lease");
        };
        let mut result = json!({
            "status": "running",
            "persona": lease.persona,
            "lease_id": lease.id.to_string(),
            "acquired_at": lease.acquired_at.to_rfc3339(),
            "held": mgr.is_held(&lease.persona),
            "recording": self.recorder.current(client_id),
        });
        if let Some(p) = self.personas.get(&lease.persona) {
            result["cpus"] = json!(p.cpus);
            result["ram_mb"] = json!(p.ram_mb);
            result["timezone"] = json!(p.timezone);
            if let Some(hw) = &p.hardware {
                result["screen"] = json!(format!("{}x{}", hw.screen_width, hw.screen_height));
                result["dpr"] = json!(hw.dpr);
            }
            if let Some(id) = &p.identity {
                result["locale"] = json!(id.locale);
                result["keyboard"] = json!(id.keyboard_layout);
            }
        }
        ToolResult::ok(result)
    }

    fn require_lease_then<F>(&self, client_id: &str, f: F) -> ToolResult
    where
        F: FnOnce() -> ToolResult,
    {
        let mgr = self.lease_mgr.lock().unwrap();
        if !mgr.holder_is(client_id) {
            return ToolResult::err("no active lease; call persona_acquire first");
        }

        let handoff_mgr = self.handoff_mgr.lock().unwrap();
        if let Some(persona) = mgr.persona_for(client_id) {
            if handoff_mgr.active_for_persona(persona).is_some() {
                return ToolResult::err("handoff in progress; agent input paused");
            }
        }
        drop(handoff_mgr);
        drop(mgr);

        f()
    }

    fn handle_persona_list(&self) -> ToolResult {
        let list: Vec<Value> = self
            .personas
            .list()
            .iter()
            .map(|p| {
                let mut entry = json!({
                    "name": p.name,
                    "cpus": p.cpus,
                    "ram_mb": p.ram_mb,
                    "timezone": p.timezone,
                });
                if let Some(hw) = &p.hardware {
                    entry["screen"] = json!(format!("{}x{}", hw.screen_width, hw.screen_height));
                    entry["dpr"] = json!(hw.dpr);
                }
                if let Some(id) = &p.identity {
                    entry["locale"] = json!(id.locale);
                }
                entry
            })
            .collect();
        ToolResult::ok(json!({ "personas": list }))
    }

    fn handle_persona_acquire(&self, persona_id: &str, client_id: &str) -> ToolResult {
        let profile = match persona::profile_dir(self.personas.home(), persona_id) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(&e.to_string()),
        };
        let mut mgr = self.lease_mgr.lock().unwrap();
        let mut result = if let Some(lease) = mgr.lease_for(client_id).filter(|l| l.persona == persona_id) {
            json!({
                "status": "already_held_by_you",
                "lease_id": lease.id.to_string(),
                "persona": lease.persona,
            })
        } else {
            // one desktop, one browser: a second persona would pull the browser from under the first
            if let (Some(_), Some(other)) = (&self.local, mgr.any()) {
                return ToolResult::err(&format!(
                    "this desktop runs one persona at a time and '{}' is held by '{}'; release it first",
                    other.persona, other.agent_id
                ));
            }
            match mgr.acquire(persona_id, client_id) {
                Ok(lease) => json!({
                    "status": "acquired",
                    "lease_id": lease.id.to_string(),
                    "persona": lease.persona,
                }),
                Err(e) => return ToolResult::err(&e.to_string()),
            }
        };
        drop(mgr);
        if let Some(local) = &self.local {
            let active = self.personas.home().join("run").join("active-persona");
            match local.use_profile(persona_id, &profile, &active) {
                Ok(restarted) => {
                    result["profile"] = json!(profile);
                    result["browser"] = json!(if restarted { "restarted on this profile" } else { "unchanged" });
                }
                Err(e) => {
                    if result["status"] == "acquired" {
                        let _ = self.lease_mgr.lock().unwrap().release(persona_id, client_id);
                    }
                    return ToolResult::err(&format!("could not open {}: {e:#}", profile.display()));
                }
            }
        }
        ToolResult::ok(result)
    }

    /// A new persona from the default one's settings; no lease, and no other persona is touched.
    fn persona_create(
        &self,
        name: &str,
        timezone: &Option<String>,
        locale: &Option<String>,
        keyboard_layout: &Option<String>,
        languages: &Option<Vec<String>>,
        accept_languages: &Option<String>,
    ) -> Res {
        let mut p = persona::template(name);
        let id = p.identity.as_mut().context("the persona template has no identity")?;
        if let Some(v) = locale {
            id.locale = v.clone();
        }
        if let Some(v) = keyboard_layout {
            id.keyboard_layout = v.clone();
        }
        if let Some(v) = languages.as_ref().filter(|l| !l.is_empty()) {
            id.languages = v.clone();
            p.browser.accept_languages = v.join(",");
        }
        if let Some(v) = accept_languages {
            p.browser.accept_languages = v.clone();
        }
        if let Some(v) = timezone {
            p.timezone = v.clone();
        }
        let warnings: Vec<Value> = crate::consistency::check_persona(&p)
            .checks
            .into_iter()
            .filter(|c| !c.passed)
            .map(|c| json!({ "check": c.name, "detail": c.detail }))
            .collect();
        let path = self.personas.create(p)?;
        Ok(ToolResult::ok(json!({
            "created": name,
            "path": path,
            "profile": persona::profile_dir(self.personas.home(), name)?,
            "warnings": warnings,
        })))
    }

    fn handle_persona_release(&self, client_id: &str) -> ToolResult {
        let mut mgr = self.lease_mgr.lock().unwrap();
        if let Some(persona) = mgr.persona_for(client_id).map(|s| s.to_string()) {
            match mgr.release(&persona, client_id) {
                Ok(()) => ToolResult::ok(json!({ "status": "released" })),
                Err(e) => ToolResult::err(&e.to_string()),
            }
        } else {
            ToolResult::err("no active lease to release")
        }
    }

    fn handle_handoff_start(&self, client_id: &str, reason: &str) -> ToolResult {
        let mgr = self.lease_mgr.lock().unwrap();
        let persona = match mgr.persona_for(client_id) {
            Some(p) => p.to_string(),
            None => return ToolResult::err("no active lease; call persona_acquire first"),
        };
        drop(mgr);

        let mut handoff = self.handoff_mgr.lock().unwrap();
        if handoff.active_for_persona(&persona).is_some() {
            return ToolResult::err("handoff already active for this persona");
        }
        let session = handoff.start(&persona, reason, 600);
        ToolResult::ok(json!({
            "id": session.id.to_string(),
            "view_url": session.view_url,
            "status": "pending",
        }))
    }

    fn handle_handoff_wait(&self, id: &str, timeout_s: Option<u64>) -> ToolResult {
        let uuid = match Uuid::parse_str(id) {
            Ok(u) => u,
            Err(_) => return ToolResult::err("invalid handoff id"),
        };
        let mut handoff = self.handoff_mgr.lock().unwrap();
        match handoff.get(&uuid) {
            Some(session) => {
                let _ = timeout_s;
                let terminal = session.is_terminal();
                let status = match session.status {
                    HandoffStatus::Pending => "pending",
                    HandoffStatus::Done => "done",
                    HandoffStatus::Aborted => "aborted",
                    HandoffStatus::Expired => "expired",
                };
                ToolResult::ok(json!({ "status": status, "terminal": terminal }))
            }
            None => ToolResult::err("handoff not found"),
        }
    }

    fn handle_handoff_resolve(&self, id: &str, action: &str) -> ToolResult {
        let uuid = match Uuid::parse_str(id) {
            Ok(u) => u,
            Err(_) => return ToolResult::err("invalid handoff id"),
        };
        let status = match action {
            "abort" => HandoffStatus::Aborted,
            _ => HandoffStatus::Done,
        };
        let mut handoff = self.handoff_mgr.lock().unwrap();
        match handoff.resolve(&uuid, status) {
            Some(session) => {
                let s = match session.status {
                    HandoffStatus::Pending => "pending",
                    HandoffStatus::Done => "done",
                    HandoffStatus::Aborted => "aborted",
                    HandoffStatus::Expired => "expired",
                };
                ToolResult::ok(json!({ "status": s }))
            }
            None => ToolResult::err("handoff not found"),
        }
    }
}

/// Which hand an input tool uses: Some(true) keyboard, Some(false) mouse. None for tools that
/// only look or wait, which let the idle hand keep moving.
fn hand_use(call: &ToolCall) -> Option<bool> {
    match call {
        ToolCall::Type { .. } | ToolCall::Key { .. } | ToolCall::KeyDown { .. }
        | ToolCall::KeyUp { .. } | ToolCall::HoldKey { .. } | ToolCall::OpenUrl { .. } => Some(true),
        ToolCall::Click { .. } | ToolCall::Move { .. } | ToolCall::Drag { .. }
        | ToolCall::Scroll { .. } | ToolCall::MouseDown { .. } | ToolCall::MouseUp { .. } => Some(false),
        _ => None,
    }
}

fn settle(r: Res) -> ToolResult {
    r.unwrap_or_else(|e| ToolResult::err(&format!("{e:#}")))
}

fn tool_name(call: &ToolCall) -> String {
    serde_json::to_value(call)
        .ok()
        .and_then(|v| v.get("tool").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

/// Call arguments as stored in a recording: file payloads dropped, everything else kept.
fn record_args(call: &ToolCall) -> Value {
    let mut v = serde_json::to_value(call).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("tool");
        if let Some(data) = o.remove("data_base64") {
            o.insert("data_bytes".into(), json!(data.as_str().map_or(0, |d| d.len() * 3 / 4)));
        }
    }
    v
}

/// Polls tiny frames until nothing changes for a quiet window, or `cap` passes.
fn wait_settled(local: &LocalExecutor, cap: Duration) -> anyhow::Result<bool> {
    use std::hash::{Hash, Hasher};
    const POLL: Duration = Duration::from_millis(100);
    const QUIET: Duration = Duration::from_millis(300);
    let started = Instant::now();
    let mut last: Option<u64> = None;
    let mut stable_since = Instant::now();
    loop {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        local.diff_frame()?.hash(&mut hasher);
        let h = hasher.finish();
        if last != Some(h) {
            last = Some(h);
            stable_since = Instant::now();
        } else if stable_since.elapsed() >= QUIET {
            return Ok(true);
        }
        if started.elapsed() >= cap {
            return Ok(false);
        }
        std::thread::sleep(POLL);
    }
}

fn safe_file_name(name: &str) -> anyhow::Result<&str> {
    let base = Path::new(name).file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if base.is_empty() || base.starts_with('.') || base != name {
        bail!("name must be a plain file name like report.pdf, got {name:?}");
    }
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::HandoffNotifyConfig;
    use crate::lease::LeaseManager;

    fn make_handler() -> ToolHandler {
        let lease_mgr = LeaseManager::new();
        let handoff_mgr = HandoffManager::new(
            "http://localhost:6080/vnc.html?autoconnect=1&resize=scale".into(),
            HandoffNotifyConfig::default(),
        );
        let home = tempfile::tempdir().unwrap().keep();
        let personas = Arc::new(PersonaRegistry::load(&home).unwrap());
        let recorder = Arc::new(Recorder::open(&home, "http://localhost:3456".into()).unwrap());
        ToolHandler::new(lease_mgr, handoff_mgr, personas, recorder)
    }

    #[test]
    fn action_without_lease_rejected() {
        let handler = make_handler();
        let result = handler.handle(
            ToolCall::Screenshot {
                frame_id: None,
                max_edge: None,
                region: None,
            },
            "agent-1",
        );
        assert!(result.is_error);
        assert!(result.error.unwrap().contains("no active lease"));
    }

    #[test]
    fn acquire_then_act() {
        let handler = make_handler();
        let acq = handler.handle(
            ToolCall::PersonaAcquire { id: "alice".into() },
            "agent-1",
        );
        assert!(!acq.is_error);

        let result = handler.handle(
            ToolCall::Screenshot {
                frame_id: None,
                max_edge: None,
                region: None,
            },
            "agent-1",
        );
        assert!(!result.is_error);
    }

    #[test]
    fn session_is_recorded_from_acquire_to_release() {
        let handler = make_handler();
        assert!(handler.handle(ToolCall::RecordingGet {
            id: None, step: None, from_step: None, limit: None, frames: false,
        }, "a").is_error);

        handler.handle(ToolCall::PersonaAcquire { id: "rec".into() }, "a");
        let again = handler.handle(ToolCall::PersonaAcquire { id: "rec".into() }, "a");
        assert_eq!(again.content["status"], "already_held_by_you");
        handler.handle(ToolCall::Key { combo: "ctrl+l".into() }, "a");
        let share = handler.handle(ToolCall::RecordingShare { id: None, expires_in_s: None }, "a");
        assert!(share.content["url"].as_str().unwrap().contains("/recordings/"));
        handler.handle(ToolCall::PersonaRelease, "a");

        let list = handler.handle(ToolCall::RecordingList { persona: None, limit: None }, "a");
        let rec = &list.content["recordings"][0];
        assert_eq!(rec["steps"], 5);
        assert!(rec["ended_at"].is_string());

        let page = handler.handle(ToolCall::RecordingGet {
            id: rec["id"].as_str().map(str::to_string), step: Some(3), from_step: None, limit: None, frames: false,
        }, "a");
        assert_eq!(page.content["steps"][0]["tool"], "key");
        assert_eq!(page.content["steps"][0]["args"]["combo"], "ctrl+l");
    }

    #[test]
    fn persona_create_is_listed_and_acquirable_without_restart() {
        let handler = make_handler();
        let created = handler.handle(
            ToolCall::PersonaCreate {
                name: "work".into(),
                timezone: Some("Europe/Berlin".into()),
                locale: None,
                keyboard_layout: None,
                languages: Some(vec!["de".into(), "en".into()]),
                accept_languages: None,
            },
            "a",
        );
        assert!(!created.is_error, "{:?}", created.error);
        assert_eq!(created.content["warnings"], json!([]));
        let list = handler.handle(ToolCall::PersonaList, "a");
        assert_eq!(list.content["personas"][0]["name"], "work");
        assert_eq!(list.content["personas"][0]["timezone"], "Europe/Berlin");
        assert!(!handler.handle(ToolCall::PersonaAcquire { id: "work".into() }, "a").is_error);
        assert_eq!(handler.handle(ToolCall::PersonaStatus, "a").content["timezone"], "Europe/Berlin");

        let again = handler.handle(
            ToolCall::PersonaCreate {
                name: "work".into(), timezone: None, locale: None, keyboard_layout: None,
                languages: None, accept_languages: None,
            },
            "b",
        );
        assert!(again.error.unwrap().contains("already exists"));
    }

    #[test]
    fn persona_names_cannot_escape() {
        let handler = make_handler();
        for bad in ["../x", "a/b", "", "a b"] {
            assert!(handler.handle(ToolCall::PersonaAcquire { id: bad.into() }, "a").is_error, "{bad:?}");
            let created = handler.handle(
                ToolCall::PersonaCreate {
                    name: bad.into(), timezone: None, locale: None, keyboard_layout: None,
                    languages: None, accept_languages: None,
                },
                "a",
            );
            assert!(created.is_error, "{bad:?}");
        }
    }

    #[test]
    fn file_names_cannot_escape() {
        assert!(safe_file_name("report.pdf").is_ok());
        assert!(safe_file_name("../x").is_err());
        assert!(safe_file_name("a/b.txt").is_err());
        assert!(safe_file_name(".bashrc").is_err());
    }

    #[test]
    fn handoff_pauses_agent_input() {
        let handler = make_handler();
        handler.handle(
            ToolCall::PersonaAcquire { id: "bob".into() },
            "agent-2",
        );
        handler.handle(
            ToolCall::HandoffStart { reason: "CAPTCHA".into() },
            "agent-2",
        );
        let result = handler.handle(
            ToolCall::Click {
                x: 100,
                y: 200,
                button: "left".into(),
                count: 1,
                describe: "test".into(),
                frame_id: None,
            },
            "agent-2",
        );
        assert!(result.is_error);
        assert!(result.error.unwrap().contains("handoff in progress"));
    }

    #[test]
    fn computer_tool_dispatched() {
        let handler = make_handler();
        handler.handle(
            ToolCall::PersonaAcquire { id: "charlie".into() },
            "agent-3",
        );
        let result = handler.handle(
            ToolCall::Computer {
                action: "screenshot".into(),
                coordinate: None,
                text: None,
                extra: Default::default(),
            },
            "agent-3",
        );
        assert!(!result.is_error);
    }
}
