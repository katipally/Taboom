use crate::consistency::check_applied;
use crate::audit::AuditLog;
use crate::cdp::{http_url_host, PageHost, PageScheme, PageTargets};
use crate::lease::LeaseManager;
use crate::local::{finish_video, png_size, LocalExecutor, OcrTimedOut, Rect, ScaledCrop, Screen};
use crate::persona::Persona;
use crate::recording::{self, Recorder};
use crate::route::Monitor;
use crate::tools::{ToolCall, ToolRegion, translate_computer_tool};
use crate::vault::{domain_matches, SecretType, Vault};
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
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

/// Longest edge of screenshots by default. Larger images get downscaled by the model's API,
/// which silently skews the coordinates it sends back.
const DEFAULT_MAX_EDGE: u32 = 1280;
const PASTE_THRESHOLD_CHARS: usize = 400;
const FILE_GET_LIMIT: u64 = 20 * 1024 * 1024;
const RECORD_FRAME_DELAY: Duration = Duration::from_millis(400);
/// Full-screen OCR on a 4K persona with two language packs can take several seconds.
const OCR_TIMEOUT: Duration = Duration::from_secs(15);
const OCR_POLL_INTERVAL: Duration = Duration::from_millis(25);
const OCR_CACHE_REGIONS: usize = 8;
const ADDRESS_BAR_MIN_OCR_CONFIDENCE: f64 = 70.0;

/// The pixel space of the latest full screenshot a client saw.
#[derive(Debug, Clone, Copy)]
struct View {
    id: Uuid,
    sx: f64,
    sy: f64,
    screen: Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct OcrRegion {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Debug, Clone)]
struct OcrLine {
    text: String,
    /// Absolute native screen coordinates, mapped into the current screenshot View at output.
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    conf: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddressBarRefusal {
    Unavailable,
    Unreadable,
    LowConfidence,
    InsecurePage,
    HostMismatch,
    OtherSitesOpen,
}

#[derive(Default)]
struct OcrCache {
    by_region: HashMap<Option<OcrRegion>, CachedOcr>,
    lru: VecDeque<Option<OcrRegion>>,
}

struct CachedOcr {
    frame_hash: [u8; 32],
    lines: Vec<OcrLine>,
}

impl OcrCache {
    fn get(&mut self, region: Option<OcrRegion>, frame_hash: [u8; 32]) -> Option<Vec<OcrLine>> {
        let matches = self.by_region.get(&region).map(|cached| cached.frame_hash == frame_hash)?;
        if !matches {
            self.by_region.remove(&region);
            self.lru.retain(|cached_region| *cached_region != region);
            return None;
        }

        self.lru.retain(|cached_region| *cached_region != region);
        self.lru.push_back(region);
        self.by_region.get(&region).map(|cached| cached.lines.clone())
    }

    fn insert(&mut self, region: Option<OcrRegion>, frame_hash: [u8; 32], lines: Vec<OcrLine>) {
        self.lru.retain(|cached_region| *cached_region != region);
        self.lru.push_back(region);
        self.by_region.insert(region, CachedOcr { frame_hash, lines });
        while self.by_region.len() > OCR_CACHE_REGIONS {
            if let Some(oldest) = self.lru.pop_front() {
                self.by_region.remove(&oldest);
            }
        }
    }

    fn clear(&mut self) {
        self.by_region.clear();
        self.lru.clear();
    }
}

impl View {
    fn fit(screen: Screen, max_edge: u32) -> f64 {
        (max_edge as f64 / screen.width.max(screen.height) as f64).min(1.0)
    }

    /// Grim's scale is relative to Sway layout coordinates, while fit is relative to physical
    /// mode pixels. Multiply by output scale to preserve the requested image dimensions.
    fn capture_scale(screen: Screen, max_edge: u32) -> f64 {
        screen.scale * Self::fit(screen, max_edge)
    }
}

pub struct ToolHandler {
    tool_lock: Mutex<()>,
    lease_mgr: Mutex<LeaseManager>,
    persona: Arc<Persona>,
    route: Arc<Monitor>,
    local: LocalExecutor,
    recorder: Arc<Recorder>,
    views: Mutex<HashMap<String, View>>,
    ocr_cache: Mutex<OcrCache>,
    videos: Mutex<HashMap<String, (std::process::Child, PathBuf)>>,
    files_dir: PathBuf,
    max_edge: u32,
    secret_context: Option<SecretContext>,
    visual_capture: Arc<Mutex<()>>,
    visual_sensitive: Arc<AtomicBool>,
    sensitive_marker: Option<PathBuf>,
}

struct SecretContext {
    vault: Arc<Vault>,
    audit: Arc<AuditLog>,
    page_targets: Arc<dyn PageTargets>,
}

impl ToolHandler {
    pub fn new(persona: Arc<Persona>, route: Arc<Monitor>, recorder: Arc<Recorder>, local: LocalExecutor) -> Self {
        local.start_idle();
        let files_dir = std::env::var_os("TABOOM_FILES_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads")))
            .unwrap_or_else(|| PathBuf::from("Downloads"));
        let max_edge = std::env::var("TABOOM_MAX_EDGE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_EDGE);
        Self {
            tool_lock: Mutex::new(()),
            lease_mgr: Mutex::new(LeaseManager::default()),
            persona,
            route,
            local,
            recorder,
            views: Mutex::new(HashMap::new()),
            ocr_cache: Mutex::new(OcrCache::default()),
            videos: Mutex::new(HashMap::new()),
            files_dir,
            max_edge,
            secret_context: None,
            visual_capture: Arc::new(Mutex::new(())),
            visual_sensitive: Arc::new(AtomicBool::new(false)),
            sensitive_marker: None,
        }
    }

    pub fn with_secret_context(
        mut self,
        vault: Arc<Vault>,
        audit: Arc<AuditLog>,
        page_targets: Arc<dyn PageTargets>,
        home: &Path,
    ) -> Result<Self> {
        let marker = home.join("run").join("secret-visuals-disabled");
        self.visual_sensitive.store(marker.exists(), Ordering::SeqCst);
        self.secret_context = Some(SecretContext { vault, audit, page_targets });
        self.sensitive_marker = Some(marker);
        Ok(self)
    }

    /// Ends the active session at daemon shutdown: keys up, video finalized, recording closed.
    /// Waits for a running tool first; a poisoned lock still guards.
    pub fn shutdown(&self) {
        let _tool = self.tool_lock.lock();
        let Some(client) = self.lease_mgr.lock().unwrap_or_else(|e| e.into_inner()).holder().map(|l| l.agent_id.clone()) else {
            return;
        };
        self.local.set_idle(false);
        let _ = self.local.release_all();
        if let Some((child, mkv)) = self.videos.lock().unwrap_or_else(|e| e.into_inner()).remove(&client) {
            if let Err(e) = finish_video(child, &mkv) {
                tracing::warn!("video not finalized at shutdown: {e:#}");
            }
        }
        self.recorder.stop(&client);
    }

    /// vinput answers a position query and sway reports an active output.
    pub fn health(&self) -> Vec<(&'static str, Result<()>)> {
        vec![("vinput", self.local.cursor().map(drop)), ("sway", self.local.output().map(drop))]
    }

    pub fn recorder(&self) -> &Recorder {
        &self.recorder
    }

    /// Runs one tool call and records it in the caller's session.
    pub fn handle(&self, call: ToolCall, client_id: &str) -> ToolResult {
        // An input action and its recording/redaction decision form one serialized transaction.
        // This prevents another MCP call from taking a screenshot or interleaving a key press
        // while sensitive text is in flight.
        let _tool = self.tool_lock.lock().unwrap();
        let call = match &call {
            ToolCall::Computer { .. } => match translate_computer_tool(&call) {
                Some(native) => native,
                None => return ToolResult::err("unsupported computer action"),
            },
            _ => call,
        };

        let started = Instant::now();
        let result = self.dispatch(&call, client_id);

        if let (ToolCall::SessionStart, false) = (&call, result.is_error) {
            match self.recorder.start(client_id, &self.persona.name) {
                Ok(rec) => self.start_video(client_id, &rec),
                Err(e) => tracing::warn!("recording not started: {e}"),
            }
            self.local.set_idle(true);
        }

        let frame = if self.visual_sensitive.load(Ordering::SeqCst) {
            recording::Frame::None
        } else { match (&call, result.is_error) {
            (ToolCall::Screenshot { .. } | ToolCall::Zoom { .. }, false) => result
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
                _,
            ) => recording::Frame::Pending("png"),
            _ => recording::Frame::None,
        }};
        let pending = self.recorder.record(
            client_id,
            &tool_name(&call),
            record_args(&call),
            result.error.as_deref(),
            started.elapsed().as_millis() as u64,
            frame,
        );
        if let Some(path) = pending {
            let local = self.local.clone();
            let visual_capture = Arc::clone(&self.visual_capture);
            let visual_sensitive = Arc::clone(&self.visual_sensitive);
            std::thread::spawn(move || {
                std::thread::sleep(RECORD_FRAME_DELAY);
                let _visual = visual_capture.lock().unwrap();
                if visual_sensitive.load(Ordering::SeqCst) {
                    return;
                }
                let capture = local.screen().and_then(|screen| local.capture(None, screen.scale * 0.5, "png"));
                match capture {
                    Ok(jpg) => {
                        let _ = std::fs::write(path, jpg);
                    }
                    Err(e) => tracing::debug!("recording frame skipped: {e}"),
                }
            });
        }

        if let (ToolCall::SessionEnd, false) = (&call, result.is_error) {
            self.local.set_idle(false);
            let _ = self.local.release_all();
            self.stop_video(client_id);
            self.recorder.stop(client_id);
            self.views.lock().unwrap().remove(client_id);
        }
        result
    }

    /// Screen video for the session, next to its events.
    fn start_video(&self, client_id: &str, rec: &str) {
        if self.visual_sensitive.load(Ordering::SeqCst) {
            return;
        }
        let Ok(dir) = self.recorder.session_path(rec) else {
            return;
        };
        let mut videos = self.videos.lock().unwrap();
        if videos.contains_key(client_id) {
            return;
        }
        let mkv = dir.join("video.mkv");
        match self.local.start_video(&mkv) {
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

    fn stop_video_before_secret(&self, client_id: &str) -> Result<()> {
        if let Some((child, mkv)) = self.videos.lock().unwrap().remove(client_id) {
            finish_video(child, &mkv).context("could not stop the screen recording before secret typing")?;
        }
        Ok(())
    }

    /// OCRs only the geometry-derived Chrome omnibox crop. The bytes and recognized text stay in
    /// this stack frame; unlike ordinary OCR, this result is never cached, returned, or logged.
    fn cross_check_visible_omnibox(&self, page_hosts: &[PageHost]) -> std::result::Result<String, AddressBarRefusal> {
        let crop: ScaledCrop = self.local.focused_chrome_omnibox_crop()
            .map_err(|_| AddressBarRefusal::Unavailable)?;
        let png = self.local.capture_cursor_free(Some(crop.rect), crop.scale, "png")
            .map_err(|_| AddressBarRefusal::Unavailable)?;
        if !scaled_crop_png_size_matches(&png, crop) {
            return Err(AddressBarRefusal::Unavailable);
        }
        let installed = installed_tesseract_languages(None)
            .map_err(|_| AddressBarRefusal::Unavailable)?;
        let language = select_tesseract_languages(&self.persona.identity.languages, &installed)
            .map_err(|_| AddressBarRefusal::Unavailable)?;
        let tsv = run_tesseract(png, &language, None)
            .map_err(|_| AddressBarRefusal::Unavailable)?;
        let origin = (
            (crop.rect.x as f64 * crop.scale).round() as u32,
            (crop.rect.y as f64 * crop.scale).round() as u32,
        );
        let lines = parse_tesseract_tsv(&tsv, origin)
            .map_err(|_| AddressBarRefusal::Unavailable)?;
        verify_visible_omnibox_lines(&lines, page_hosts)
    }

    fn type_secret(
        &self,
        client_id: &str,
        parts: &[SecretPart],
        mode: &str,
        submit: bool,
    ) -> Res {
        if mode == "paste" {
            bail!("secret placeholders cannot use paste mode because it would retain plaintext in the clipboard");
        }
        let ctx = self.secret_context.as_ref()
            .context("vault secret typing is not configured")?;
        let names: Vec<&str> = parts.iter().filter_map(|part| match part {
            SecretPart::Secret(name) => Some(name.as_str()),
            SecretPart::Text(_) => None,
        }).collect();
        if names.is_empty() {
            bail!("secret placeholder is empty");
        }

        let focused = self.local.focused_chrome_is_fullscreen();
        match focused {
            Ok(false) => {}
            Ok(true) => return Err(refuse_secrets(ctx, &names, "unknown", &[], "fullscreen", "secret typing is refused while Chrome is fullscreen")),
            Err(_) => return Err(refuse_secrets(ctx, &names, "unknown", &[], "chrome_not_focused", "secret typing requires a focused Chrome window")),
        }
        if ctx.vault.status() != "unlocked" {
            return Err(refuse_secrets(ctx, &names, "unknown", &[], "vault_locked", "vault is locked; unlock it before typing secrets"));
        }
        let page_hosts = match ctx.page_targets.page_hosts() {
            Ok(page_hosts) => page_hosts,
            Err(e) if e.is::<crate::cdp::OpaquePageOpen>() => return Err(refuse_secrets(ctx, &names, "unknown", &[], "opaque_page_open", "a blank, data: or file: page is open in a tab or popup; close it so only the login site remains, then retry")),
            Err(_) => return Err(refuse_secrets(ctx, &names, "unknown", &[], "current_domain_unavailable", "could not verify the current Chrome page; secret typing was refused")),
        };

        // CDP supplies candidate HTTPS/HTTP page hosts. The private visible-omnibox OCR selects
        // the host for the focused Chrome window before any vault value is opened.
        let domain = match self.cross_check_visible_omnibox(&page_hosts) {
            Ok(domain) => domain,
            Err(refusal) => return Err(refuse_address_bar(ctx, &names, "unknown", refusal)),
        };

        let metadata = allowed_secret_metadata(ctx, &names, &domain)?;

        // Resolve only after vault, page, fullscreen, and allow-list checks pass. The expanded
        // value exists only in this stack frame and is never included in an error, trace, or log.
        let mut expanded = String::new();
        let mut resolved_names = Vec::new();
        for part in parts {
            match part {
                SecretPart::Text(text) => expanded.push_str(text),
                SecretPart::Secret(name) => {
                    let Some((expected_type, allowed)) = metadata.get(name) else {
                        bail!("secret metadata changed during validation");
                    };
                    let record = match ctx.vault.get_secret(name) {
                        Ok(Some(record)) => record,
                        Ok(None) => return Err(refuse_secrets(ctx, &[name], &domain, allowed, "secret_missing", "vault secret was not found")),
                        Err(_) => return Err(refuse_secrets(ctx, &[name], &domain, allowed, "vault_locked", "vault is locked; unlock it before typing secrets")),
                    };
                    if &record.secret_type != expected_type || &record.allowed_domains != allowed {
                        return Err(refuse_secrets(ctx, &[name], &domain, allowed, "secret_changed", "vault secret changed during validation; retry the request"));
                    }
                    let value = resolve_secret_value(&record, std::time::SystemTime::now())
                        .map_err(|_| refuse_secrets(ctx, &[name], &domain, allowed, "secret_value_invalid", "vault secret could not be typed"))?;
                    expanded.push_str(&value);
                    resolved_names.push(name.clone());
                }
            }
        }
        if !self.local.can_type(&expanded)? {
            return Err(refuse_secrets(ctx, &names, &domain, &[], "unsupported_layout_character", "secret text contains a character unavailable on this keyboard layout"));
        }

        // Wait for any already scheduled safe frame capture. Keep video running until the final
        // focus/domain/route check passes, then close it just before redaction and key input.
        let _visual = self.visual_capture.lock().unwrap();
        let trace = self.local.pause_trace()?;
        let check_focus_route_and_domain = || -> Result<String> {
            match self.local.focused_chrome_is_fullscreen() {
                Ok(false) => {}
                Ok(true) => return Err(refuse_secrets(ctx, &names, &domain, &[], "fullscreen", "secret typing is refused while Chrome is fullscreen")),
                Err(_) => return Err(refuse_secrets(ctx, &names, &domain, &[], "chrome_not_focused", "secret typing requires a focused Chrome window")),
            }
            if self.route_refusal().is_some() {
                return Err(refuse_secrets(ctx, &names, &domain, &[], "route_unhealthy", "the route check failed; secret typing was refused"));
            }
            let page_hosts = ctx.page_targets.page_hosts()
                .map_err(|_| refuse_secrets(ctx, &names, &domain, &[], "current_domain_unavailable", "could not recheck the current Chrome page; secret typing was refused"))?;
            let current = self.cross_check_visible_omnibox(&page_hosts)
                .map_err(|refusal| refuse_address_bar(ctx, &names, &domain, refusal))?;
            if current == domain {
                Ok(current)
            } else {
                Err(refuse_secrets(ctx, &names, &domain, &[], "domain_changed", "Chrome navigated during secret authorization; secret typing was refused"))
            }
        };
        let typed = self.local.key_type_exact_checked(&expanded, || {
            // Recheck focus, route, CDP candidates, and the visible host before stopping video.
            check_focus_route_and_domain()?;
            self.stop_video_before_secret(client_id).map_err(|_| {
                refuse_secrets(ctx, &names, &domain, &[], "video_stop_failed", "screen recording could not be stopped; secret typing was refused")
            })?;
            // Repeat OCR after finalizing video so the selected host is checked immediately
            // before redaction and key input.
            let current_domain = check_focus_route_and_domain()?;
            for name in &resolved_names {
                ctx.audit.log_secret_used(name, &current_domain)
                    .context("could not audit secret use; secret typing was refused")?;
            }
            self.mark_visual_sensitive()?;
            Ok(())
        });
        let trace_restored = trace.resume();
        typed.context("exact secret typing failed")?;
        trace_restored.context("secret was typed, but VINPUT_TRACE could not be restored")?;
        if submit {
            self.local.key_press("Return")?;
        }
        Ok(ToolResult::ok(json!({
            "typed": true,
            "mode": "keys",
            "submitted": submit,
            "secrets_used": resolved_names.len(),
        })))
    }

    fn mark_visual_sensitive(&self) -> Result<()> {
        let marker = self.sensitive_marker.as_ref().context("persistent secret redaction marker is not configured")?;
        let mut file = OpenOptions::new().create(true).append(true).mode(0o600).open(marker)
            .context("could not persist secret redaction state")?;
        file.write_all(b"secret typing used; screen output disabled\n")?;
        file.sync_all()?;
        self.visual_sensitive.store(true, Ordering::SeqCst);
        self.ocr_cache.lock().unwrap().clear();
        Ok(())
    }

    fn dispatch(&self, call: &ToolCall, client_id: &str) -> ToolResult {
        match call {
            ToolCall::SessionStart => self.session_start(client_id),
            ToolCall::SessionEnd => match self.lease_mgr.lock().unwrap().release(client_id) {
                Ok(()) => ToolResult::ok(json!({ "status": "ended" })),
                Err(e) => ToolResult::err(&e.to_string()),
            },
            ToolCall::PersonaStatus => self.persona_status(client_id),

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

            device_call => self.require_lease_then(client_id, || {
                settle(match hand_use(device_call) {
                    Some(on_keyboard) => self.local.exclusive(Some(on_keyboard), || {
                        self.local_call(&self.local, device_call, client_id)
                    }),
                    None => self.local_call(&self.local, device_call, client_id),
                })
            }),
        }
    }

    /// Screen, input, files and clipboard against the container's own desktop.
    fn local_call(&self, local: &LocalExecutor, call: &ToolCall, client_id: &str) -> Res {
        match call {
            ToolCall::Screenshot { max_edge, region, .. } => {
                if self.visual_sensitive.load(Ordering::SeqCst) {
                    bail!("screen images and video are disabled for this data volume after vault secret typing; clear the persistent data volume to reset this state");
                }
                let screen = local.screen()?;
                let max_edge = max_edge.unwrap_or(self.max_edge).clamp(200, 4096);
                if let Some(region) = region {
                    return self.zoom(local, client_id, region, max_edge);
                }
                let png = local.capture(None, View::capture_scale(screen, max_edge), "png")?;
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
            ToolCall::Zoom { region } => {
                if self.visual_sensitive.load(Ordering::SeqCst) {
                    bail!("screen images and video are disabled for this data volume after vault secret typing; clear the persistent data volume to reset this state");
                }
                self.zoom(local, client_id, region, 4096)
            }
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
                if let Some(parts) = parse_secret_text(text)? {
                    return self.type_secret(client_id, &parts, mode, *submit);
                }
                let typeable = local.can_type(text)?;
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
            ToolCall::FindText { text, region } => {
                let (view, lines) = self.ocr_lines(local, client_id, region.as_ref(), None)?;
                let needle = normalize_ocr_text(text);
                if needle.is_empty() {
                    bail!("text must not be empty");
                }
                let matches: Vec<Value> = lines
                    .iter()
                    .filter(|line| normalize_ocr_text(&line.text).contains(&needle))
                    .map(|line| ocr_line_json(&view, line))
                    .collect();
                Ok(ToolResult::ok(json!(matches)))
            }
            ToolCall::ReadText { region } => {
                let (view, lines) = self.ocr_lines(local, client_id, region.as_ref(), None)?;
                Ok(ToolResult::ok(json!(lines.iter().map(|line| ocr_line_json(&view, line)).collect::<Vec<_>>())))
            }
            ToolCall::Wait { ms, until_settled, until_text } => {
                let started = Instant::now();
                if until_settled.unwrap_or(false) && until_text.is_some() {
                    bail!("choose either until_settled or until_text");
                }
                if until_settled.unwrap_or(false) && self.visual_sensitive.load(Ordering::SeqCst) {
                    bail!("screen images, OCR and video are disabled for this data volume after vault secret typing; clear the persistent data volume to reset this state");
                }
                if let Some(text) = until_text {
                    let needle = normalize_ocr_text(text);
                    if needle.is_empty() {
                        bail!("until_text must not be empty");
                    }
                    let cap = Duration::from_millis(ms.unwrap_or(3000).min(60_000));
                    let deadline = started + cap;
                    loop {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            return Ok(ToolResult::ok(json!({
                                "found": false,
                                "until_text": text,
                                "matches": [],
                                "waited_ms": started.elapsed().as_millis() as u64,
                            })));
                        }
                        let (view, lines) = match self.ocr_lines(local, client_id, None, Some(deadline)) {
                            Ok(result) => result,
                            Err(error) if error.is::<OcrTimedOut>() => {
                                let remaining = deadline.saturating_duration_since(Instant::now());
                                if remaining.is_zero() {
                                    return Ok(ToolResult::ok(json!({
                                        "found": false,
                                        "until_text": text,
                                        "matches": [],
                                        "waited_ms": started.elapsed().as_millis() as u64,
                                    })));
                                }
                                std::thread::sleep(Duration::from_millis(200).min(remaining));
                                continue;
                            }
                            Err(error) => return Err(error),
                        };
                        let matches: Vec<Value> = lines
                            .iter()
                            .filter(|line| normalize_ocr_text(&line.text).contains(&needle))
                            .map(|line| ocr_line_json(&view, line))
                            .collect();
                        if Instant::now() < deadline && !matches.is_empty() {
                            return Ok(ToolResult::ok(json!({
                                "found": true,
                                "until_text": text,
                                "matches": matches,
                                "waited_ms": started.elapsed().as_millis() as u64,
                            })));
                        }
                        let elapsed = started.elapsed();
                        if Instant::now() >= deadline {
                            return Ok(ToolResult::ok(json!({
                                "found": false,
                                "until_text": text,
                                "matches": [],
                                "waited_ms": elapsed.as_millis() as u64,
                            })));
                        }
                        std::thread::sleep(Duration::from_millis(200).min(deadline.saturating_duration_since(Instant::now())));
                    }
                }
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
        let capture_scale = s.scale * (max_edge as f64 / w.max(h) as f64).min(1.0);
        let png = local.capture(Some(layout_rect_for_physical(rect, s.scale)?), capture_scale, "png")?;
        let (iw, ih) = png_size(&png).context("grim returned a non-PNG image")?;
        Ok(ToolResult::ok(json!({
            "width": iw,
            "height": ih,
            "screen_region": { "x": x, "y": y, "w": w, "h": h },
            "note": "inspection only; keep using the latest screenshot's coordinates",
        }))
        .with_image(&png, "image/png"))
    }

    /// Runs local OCR against a native-scale grim crop. OCR results stay in native screen space
    /// in memory and are transformed through the current View only when returned to a caller.
    fn ocr_lines(
        &self,
        local: &LocalExecutor,
        client_id: &str,
        region: Option<&ToolRegion>,
        deadline: Option<Instant>,
    ) -> Result<(View, Vec<OcrLine>)> {
        if self.visual_sensitive.load(Ordering::SeqCst) {
            bail!("screen images, OCR and video are disabled for this data volume after vault secret typing; clear the persistent data volume to reset this state");
        }
        // Secret typing takes the same lock before setting the persistent suppression marker.
        // Recheck under the lock so neither the screen query nor a fresh capture starts after
        // secret entry begins.
        let visual = match deadline {
            Some(_) => match self.visual_capture.try_lock() {
                Ok(visual) => visual,
                Err(std::sync::TryLockError::WouldBlock) => return Err(anyhow::Error::new(OcrTimedOut)),
                Err(std::sync::TryLockError::Poisoned(_)) => bail!("visual capture is unavailable"),
            },
            None => self.visual_capture.lock().unwrap(),
        };
        if self.visual_sensitive.load(Ordering::SeqCst) {
            bail!("screen images, OCR and video are disabled for this data volume after vault secret typing; clear the persistent data volume to reset this state");
        }
        // until_text has one absolute deadline across both Sway and Grim. The returned View is
        // reused by the caller so the wait poll never performs an extra unbounded screen query.
        let screen = match deadline {
            Some(deadline) => local.screen_with_deadline(deadline)?,
            None => local.screen()?,
        };
        let view = self.view_for_screen(client_id, screen)?;
        let requested_region = region.map(|r| ocr_region_for_view(view, r)).transpose()?;
        let cache_key = requested_region;
        let capture_region = requested_region
            .map(|r| layout_rect_for_physical(Rect { x: r.x, y: r.y, w: r.w, h: r.h }, screen.scale))
            .transpose()?;
        // Hash the exact native pixels OCR reads: a downscaled frame can hide a one-glyph change
        // and serve stale text. PPM skips PNG encoding; Tesseract reads it directly.
        let capture = || match deadline {
            Some(deadline) => local.capture_cursor_free_with_deadline(capture_region, screen.scale, "ppm", deadline),
            None => local.capture_cursor_free(capture_region, screen.scale, "ppm"),
        };
        let frame = capture()?;
        let frame_hash = image_fingerprint(&frame);
        if let Some(lines) = self.ocr_cache.lock().unwrap().get(cache_key, frame_hash) {
            return Ok((view, lines));
        }
        drop(visual);

        let installed = installed_tesseract_languages(deadline)?;
        let language = select_tesseract_languages(&self.persona.identity.languages, &installed)?;
        let tsv = run_tesseract(frame, &language, deadline)?;
        let origin = requested_region.map_or((0, 0), |_| {
            let layout = capture_region.expect("a requested OCR region has a Grim region");
            (
                (layout.x as f64 * screen.scale).round() as u32,
                (layout.y as f64 * screen.scale).round() as u32,
            )
        });
        let lines = parse_tesseract_tsv(&tsv, origin)?;

        // Cache only if the pixels stayed unchanged while Tesseract ran.
        let still_same_frame = capture().ok().is_some_and(|after| image_fingerprint(&after) == frame_hash);
        if still_same_frame && !self.visual_sensitive.load(Ordering::SeqCst) {
            self.ocr_cache.lock().unwrap().insert(cache_key, frame_hash, lines.clone());
        }
        Ok((view, lines))
    }

    /// The client's latest view, or the default one if they have not taken a screenshot yet.
    fn view(&self, local: &LocalExecutor, client_id: &str) -> anyhow::Result<View> {
        let screen = local.screen()?;
        self.view_for_screen(client_id, screen)
    }

    fn view_for_screen(&self, client_id: &str, screen: Screen) -> anyhow::Result<View> {
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
        let want = if self.visual_sensitive.load(Ordering::SeqCst) {
            0
        } else if step.is_some() {
            1
        } else if frames {
            MAX_FRAMES
        } else {
            0
        };
        for e in events.iter().filter(|e| e.frame.is_some()).take(want) {
            let name = e.frame.as_deref().unwrap_or_default();
            if let Ok(bytes) = std::fs::read(self.recorder.frame_path(&id, name)?) {
                let mime = if name.ends_with(".png") { "image/png" } else { "image/jpeg" };
                result = result.with_image(&bytes, mime);
            }
        }
        Ok(result)
    }

    fn persona_status(&self, client_id: &str) -> ToolResult {
        let applied = crate::consistency::observe(self.local.output().ok(), &self.persona);
        let failed: Vec<Value> = check_applied(&self.persona, &applied)
            .checks
            .into_iter()
            .filter(|c| !c.passed)
            .map(|c| json!({ "check": c.name, "detail": c.detail }))
            .collect();
        let mgr = self.lease_mgr.lock().unwrap();
        let session = mgr.holder().map(|l| {
            let mine = l.agent_id == client_id;
            json!({
                "held_by_you": mine,
                "lease_id": mine.then(|| l.id.to_string()),
                "started_at": l.acquired_at.to_rfc3339(),
                "recording": if mine { self.recorder.current(client_id) } else { None },
            })
        });
        ToolResult::ok(json!({
            "persona": self.persona.name,
            "declared": &*self.persona,
            "applied": applied,
            "mismatches": failed,
            "route": self.route.health(),
            "session": session,
        }))
    }

    /// One agent at a time; the layout's character map is read from vinput here, once.
    fn session_start(&self, client_id: &str) -> ToolResult {
        if let Some(refusal) = self.route_refusal() {
            return refusal;
        }
        let mut mgr = self.lease_mgr.lock().unwrap();
        if let Some(lease) = mgr.lease_for(client_id) {
            return ToolResult::ok(json!({ "status": "already_started", "lease_id": lease.id.to_string() }));
        }
        let lease_id = match mgr.acquire(client_id) {
            Ok(lease) => lease.id.to_string(),
            Err(e) => return ToolResult::err(&e.to_string()),
        };
        if let Err(e) = self.local.load_keymap() {
            let _ = mgr.release(client_id);
            return ToolResult::err(&format!("could not read the keyboard layout from vinput: {e:#}"));
        }
        self.local.set_seed(self.persona.seed());
        ToolResult::ok(json!({ "status": "started", "lease_id": lease_id, "persona": self.persona.name }))
    }

    fn route_refusal(&self) -> Option<ToolResult> {
        let health = self.route.health();
        (!health.ok).then(|| {
            ToolResult::err(&format!(
                "the {} route check failed at {}: {}; actions are refused until a check passes",
                health.route,
                health.checked_at.to_rfc3339(),
                health.detail
            ))
        })
    }

    fn require_lease_then<F>(&self, client_id: &str, f: F) -> ToolResult
    where
        F: FnOnce() -> ToolResult,
    {
        if self.lease_mgr.lock().unwrap().lease_for(client_id).is_none() {
            return ToolResult::err("no active session; call session_start first");
        }
        if let Some(refusal) = self.route_refusal() {
            return refusal;
        }
        f()
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
#[derive(Debug, PartialEq, Eq)]
enum SecretPart {
    Text(String),
    Secret(String),
}

fn allowed_secret_metadata(
    ctx: &SecretContext,
    names: &[&str],
    domain: &str,
) -> Result<HashMap<String, (SecretType, Vec<String>)>> {
    if ctx.vault.status() != "unlocked" {
        return Err(refuse_secrets(ctx, names, domain, &[], "vault_locked", "vault is locked; unlock it before typing secrets"));
    }
    let mut metadata = HashMap::new();
    for name in names {
        let entry = match ctx.vault.secret_metadata(name) {
            Ok(Some(entry)) => entry,
            Ok(None) => return Err(refuse_secrets(ctx, &[*name], domain, &[], "secret_missing", "vault secret was not found")),
            Err(_) => return Err(refuse_secrets(ctx, &[*name], domain, &[], "vault_locked", "vault is locked; unlock it before typing secrets")),
        };
        if entry.1.is_empty() {
            return Err(refuse_secrets(ctx, &[*name], domain, &entry.1, "empty_allow_list", "secret has no allowed domains"));
        }
        if !domain_matches(&entry.1, domain) {
            return Err(refuse_secrets(ctx, &[*name], domain, &entry.1, "domain_denied", "current Chrome page is not allowed to receive this secret"));
        }
        metadata.insert((*name).to_string(), entry);
    }
    Ok(metadata)
}

fn refuse_secrets(
    ctx: &SecretContext,
    names: &[&str],
    domain: &str,
    allowed: &[String],
    reason: &str,
    message: &str,
) -> anyhow::Error {
    for name in names {
        if ctx.audit.log_secret_refused(name, domain, allowed, reason).is_err() {
            return anyhow!("secret typing was refused because the audit log is unavailable");
        }
    }
    anyhow!(message.to_string())
}

fn resolve_secret_value(record: &crate::vault::SecretRecord, now: std::time::SystemTime) -> Result<String> {
    let value = std::str::from_utf8(&record.encrypted_value)
        .context("vault secret is not UTF-8")?;
    match &record.secret_type {
        SecretType::TotpSeed => crate::totp::generate_totp(value, now),
        SecretType::Password | SecretType::Note => Ok(value.to_string()),
    }
}

fn parse_secret_text(text: &str) -> Result<Option<Vec<SecretPart>>> {
    const OPEN: &str = "<secret>";
    const CLOSE: &str = "</secret>";
    if !text.contains("<secret") && !text.contains(CLOSE) {
        return Ok(None);
    }
    let mut parts = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let next_open = text[cursor..].find("<secret").map(|i| cursor + i);
        let next_close = text[cursor..].find(CLOSE).map(|i| cursor + i);
        if next_close.is_some_and(|close| next_open.map_or(true, |open| close < open)) {
            bail!("unmatched </secret> placeholder tag");
        }
        let Some(open) = next_open else {
            if cursor < text.len() {
                parts.push(SecretPart::Text(text[cursor..].to_string()));
            }
            break;
        };
        if open > cursor {
            parts.push(SecretPart::Text(text[cursor..open].to_string()));
        }
        if !text[open..].starts_with(OPEN) {
            bail!("secret placeholder must use <secret>NAME</secret>");
        }
        let name_start = open + OPEN.len();
        let close = text[name_start..].find(CLOSE).map(|i| name_start + i)
            .context("unclosed <secret> placeholder")?;
        let name = &text[name_start..close];
        if name.is_empty() || name.contains("<secret") || taboom_core::check_name(name).is_err() {
            bail!("secret placeholder name must be 1-64 letters, digits, '_' or '-'");
        }
        parts.push(SecretPart::Secret(name.to_string()));
        cursor = close + CLOSE.len();
    }
    if parts.is_empty() {
        bail!("secret placeholder is empty");
    }
    Ok(Some(parts))
}

fn wait_settled(local: &LocalExecutor, cap: Duration) -> anyhow::Result<bool> {
    use std::hash::{Hash, Hasher};
    const POLL: Duration = Duration::from_millis(100);
    const QUIET: Duration = Duration::from_millis(300);
    let started = Instant::now();
    let output_scale = local.screen()?.scale;
    let mut last: Option<u64> = None;
    let mut stable_since = Instant::now();
    loop {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        local.diff_frame_at_scale(output_scale, None)?.hash(&mut hasher);
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

fn image_fingerprint(image: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(image).into()
}

fn ocr_region_for_view(view: View, region: &ToolRegion) -> Result<OcrRegion> {
    if region.w == 0 || region.h == 0 {
        bail!("region width and height must be greater than zero");
    }
    let image_w = (view.screen.width as f64 * view.sx).round().max(1.0) as u32;
    let image_h = (view.screen.height as f64 * view.sy).round().max(1.0) as u32;
    let x0_view = region.x.min(image_w.saturating_sub(1));
    let y0_view = region.y.min(image_h.saturating_sub(1));
    let x1_view = x0_view.saturating_add(region.w).min(image_w);
    let y1_view = y0_view.saturating_add(region.h).min(image_h);
    let x0 = ((x0_view as f64 / view.sx).floor() as u32).min(view.screen.width - 1);
    let y0 = ((y0_view as f64 / view.sy).floor() as u32).min(view.screen.height - 1);
    let x1 = ((x1_view as f64 / view.sx).ceil() as u32).min(view.screen.width).max(x0 + 1);
    let y1 = ((y1_view as f64 / view.sy).ceil() as u32).min(view.screen.height).max(y0 + 1);
    Ok(OcrRegion { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}

/// Converts a physical-pixel rectangle into Grim's Sway layout-coordinate rectangle. Round
/// outward so fractional output scales never trim the OCR or zoom crop at its edges.
fn layout_rect_for_physical(rect: Rect, scale: f64) -> Result<Rect> {
    if !scale.is_finite() || scale <= 0.0 || rect.w == 0 || rect.h == 0 {
        bail!("screen region or output scale is invalid");
    }
    let x_end = rect.x.checked_add(rect.w).context("screen region x overflows")?;
    let y_end = rect.y.checked_add(rect.h).context("screen region y overflows")?;
    let edge = |pixel: u32, upper: bool| -> Result<u32> {
        let layout = pixel as f64 / scale;
        if !layout.is_finite() || layout > u32::MAX as f64 {
            bail!("screen region exceeds Grim's layout-coordinate range");
        }
        Ok(if upper { layout.ceil() as u32 } else { layout.floor() as u32 })
    };
    let x = edge(rect.x, false)?;
    let y = edge(rect.y, false)?;
    let right = edge(x_end, true)?;
    let bottom = edge(y_end, true)?;
    Ok(Rect { x, y, w: right.saturating_sub(x).max(1), h: bottom.saturating_sub(y).max(1) })
}

fn ocr_line_json(view: &View, line: &OcrLine) -> Value {
    let x0 = (line.x as f64 * view.sx).round() as u32;
    let y0 = (line.y as f64 * view.sy).round() as u32;
    let x1 = ((line.x.saturating_add(line.w)) as f64 * view.sx).round() as u32;
    let y1 = ((line.y.saturating_add(line.h)) as f64 * view.sy).round() as u32;
    json!({
        "text": line.text,
        "x": x0,
        "y": y0,
        "w": x1.saturating_sub(x0),
        "h": y1.saturating_sub(y0),
        "conf": line.conf,
    })
}

fn normalize_ocr_text(text: &str) -> String {
    let mut normalized = String::new();
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            pending_space = !normalized.is_empty();
        } else {
            if pending_space {
                normalized.push(' ');
                pending_space = false;
            }
            normalized.extend(ch.to_lowercase());
        }
    }
    normalized
}

fn scaled_crop_png_size_matches(png: &[u8], crop: ScaledCrop) -> bool {
    if !crop.scale.is_finite() || crop.scale <= 0.0 {
        return false;
    }
    let Some((width, height)) = png_size(png) else { return false };
    let expected_width = (f64::from(crop.rect.w) * crop.scale).round();
    let expected_height = (f64::from(crop.rect.h) * crop.scale).round();
    expected_width.is_finite()
        && expected_height.is_finite()
        && expected_width >= 1.0
        && expected_height >= 1.0
        && width.abs_diff(expected_width as u32) <= 1
        && height.abs_diff(expected_height as u32) <= 1
}

fn verify_visible_omnibox_lines(
    lines: &[OcrLine],
    page_hosts: &[PageHost],
) -> std::result::Result<String, AddressBarRefusal> {
    let [line] = lines else { return Err(AddressBarRefusal::Unreadable) };
    if !line.conf.is_finite() || line.conf < ADDRESS_BAR_MIN_OCR_CONFIDENCE {
        return Err(AddressBarRefusal::LowConfidence);
    }
    let (visible_host, explicit_scheme) = visible_omnibox_address(&line.text)
        .ok_or(AddressBarRefusal::Unreadable)?;
    if explicit_scheme == Some(PageScheme::Http) {
        return Err(AddressBarRefusal::InsecurePage);
    }
    let has_https = page_hosts.iter().any(|page| {
        page.scheme == PageScheme::Https && page.host == visible_host
    });
    let has_http = page_hosts.iter().any(|page| {
        page.scheme == PageScheme::Http && page.host == visible_host
    });
    if explicit_scheme.is_none() && has_http {
        // Chrome may hide the scheme in the normal omnibox display. A same-host HTTP target
        // makes that display ambiguous, even if another tab has the matching HTTPS host.
        return Err(AddressBarRefusal::InsecurePage);
    }
    if !has_https {
        return Err(AddressBarRefusal::HostMismatch);
    }
    // CDP cannot say which tab has focus without attaching to pages, and the omnibox can show
    // text that was typed but never loaded. Only when every open page is on this one host does
    // the visible host prove where the keystrokes land.
    if page_hosts.iter().any(|page| page.host != visible_host) {
        return Err(AddressBarRefusal::OtherSitesOpen);
    }
    Ok(visible_host)
}

/// Chrome may hide the `https://` prefix when the omnibox is not focused, so retain whether the
/// OCR text included a scheme and use that fact when resolving the CDP host candidates.
fn visible_omnibox_address(text: &str) -> Option<(String, Option<PageScheme>)> {
    let text = text.trim();
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return None;
    }
    let (url, scheme) = if let Some((raw_scheme, _)) = text.split_once("://") {
        let scheme = PageScheme::from_str(raw_scheme)?;
        (text.to_string(), Some(scheme))
    } else {
        if text.contains("://") {
            return None;
        }
        (format!("https://{text}"), None)
    };
    Some((http_url_host(&url)?, scheme))
}

fn refuse_address_bar(
    ctx: &SecretContext,
    names: &[&str],
    domain: &str,
    refusal: AddressBarRefusal,
) -> anyhow::Error {
    let (reason, message) = match refusal {
        AddressBarRefusal::Unavailable => (
            "address_bar_unavailable",
            "could not inspect the visible Chrome address bar; secret typing was refused",
        ),
        AddressBarRefusal::Unreadable => (
            "address_bar_unreadable",
            "the visible Chrome address bar could not be read safely; secret typing was refused",
        ),
        AddressBarRefusal::LowConfidence => (
            "address_bar_low_confidence",
            "the visible Chrome address bar OCR confidence was too low; secret typing was refused",
        ),
        AddressBarRefusal::InsecurePage => (
            "address_bar_insecure_page",
            "the visible Chrome page is not verified as HTTPS; secret typing was refused",
        ),
        AddressBarRefusal::HostMismatch => (
            "address_bar_host_mismatch",
            "the visible Chrome address bar does not match the current page; secret typing was refused",
        ),
        AddressBarRefusal::OtherSitesOpen => (
            "other_sites_open",
            "tabs or windows on other sites are open; close them so only the login site remains, then retry",
        ),
    };
    refuse_secrets(ctx, names, domain, &[], reason, message)
}

fn select_tesseract_languages(languages: &[String], installed: &HashSet<String>) -> Result<String> {
    // Tesseract's traineddata identifiers differ from BCP-47 tags. Every configured language
    // must have its matching model: silently using English for a
    // Japanese or Arabic persona, or omitting a configured secondary language, makes OCR look
    // successful while returning unreliable text.
    const MAP: &[(&str, &str)] = &[
        ("en", "eng"), ("de", "deu"), ("fr", "fra"), ("ja", "jpn"),
        ("pt", "por"), ("hi", "hin"), ("es", "spa"), ("it", "ita"),
        ("nl", "nld"), ("ru", "rus"), ("zh", "chi_sim"), ("ko", "kor"),
        ("ar", "ara"), ("sv", "swe"), ("no", "nor"), ("da", "dan"),
        ("fi", "fin"), ("pl", "pol"), ("cs", "ces"), ("ro", "ron"),
        ("hu", "hun"), ("tr", "tur"), ("uk", "ukr"), ("th", "tha"),
        ("af", "afr"), ("am", "amh"), ("as", "asm"), ("az", "aze"),
        ("be", "bel"), ("bn", "ben"), ("bo", "bod"), ("bg", "bul"),
        ("bs", "bos"), ("ca", "cat"), ("ceb", "ceb"), ("cy", "cym"),
        ("el", "ell"), ("et", "est"), ("eu", "eus"), ("fa", "fas"),
        ("fil", "fil"), ("ga", "gle"), ("gl", "glg"), ("gu", "guj"),
        ("he", "heb"), ("hr", "hrv"), ("hy", "hye"), ("id", "ind"),
        ("is", "isl"), ("jv", "jav"), ("ka", "kat"), ("kk", "kaz"),
        ("km", "khm"), ("kn", "kan"), ("ku", "kmr"), ("ky", "kir"),
        ("lo", "lao"), ("lt", "lit"), ("lv", "lav"), ("mk", "mkd"),
        ("ml", "mal"), ("mn", "mon"), ("mr", "mar"), ("ms", "msa"),
        ("mt", "mlt"), ("my", "mya"), ("ne", "nep"), ("oc", "oci"),
        ("or", "ori"), ("pa", "pan"), ("ps", "pus"), ("sa", "san"),
        ("si", "sin"), ("sk", "slk"), ("sl", "slv"), ("sq", "sqi"),
        ("sr", "srp"), ("sw", "swa"), ("ta", "tam"), ("te", "tel"),
        ("tg", "tgk"), ("tl", "tgl"), ("ur", "urd"), ("uz", "uzb"),
        ("vi", "vie"), ("yi", "yid"), ("yo", "yor"), ("zu", "zul"),
    ];
    let pack_for = |language: &str| -> Option<&'static str> {
        let mut parts = language.split(|c| c == '-' || c == '_');
        let primary = parts.next().unwrap_or_default().to_ascii_lowercase();
        let subtags: Vec<String> = parts.map(str::to_ascii_lowercase).collect();
        if primary == "zh" {
            let traditional = subtags.iter().any(|tag| matches!(tag.as_str(), "tw" | "hk" | "mo" | "hant"));
            return Some(if traditional { "chi_tra" } else { "chi_sim" });
        }
        if primary == "sr" && subtags.iter().any(|tag| tag == "latn") {
            return Some("srp_latn");
        }
        if primary == "uz" && subtags.iter().any(|tag| tag == "cyrl") {
            return Some("uzb_cyrl");
        }
        MAP.iter().find(|(tag, _)| *tag == primary.as_str()).map(|(_, pack)| *pack)
    };

    if languages.is_empty() {
        bail!("persona has no OCR languages");
    }
    let mut mapped = Vec::new();
    for language in languages {
        let pack = pack_for(language)
            .with_context(|| format!("no Tesseract traineddata mapping for persona language {language:?}"))?;
        if !installed.contains(pack) {
            bail!("persona language {language:?} requires missing Tesseract pack {pack:?}");
        }
        if !mapped.contains(&pack) {
            mapped.push(pack);
        }
    }
    Ok(mapped.join("+"))
}

fn installed_tesseract_languages(deadline: Option<Instant>) -> Result<HashSet<String>> {
    static INSTALLED: OnceLock<Mutex<Option<HashSet<String>>>> = OnceLock::new();
    let cache = INSTALLED.get_or_init(|| Mutex::new(None));
    let mut cached = match cache.try_lock() {
        Ok(cached) => cached,
        Err(std::sync::TryLockError::WouldBlock) => return Err(anyhow::Error::new(OcrTimedOut)),
        Err(std::sync::TryLockError::Poisoned(_)) => bail!("Tesseract language cache is unavailable"),
    };
    if let Some(languages) = cached.as_ref() {
        return Ok(languages.clone());
    }

    let timeout = ocr_timeout(deadline)?;
    let mut child = Command::new("tesseract")
        .arg("--list-langs")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning local tesseract OCR")?;
    let stdout = child.stdout.take().context("opening tesseract output")?;
    let reader = read_stdout_thread(stdout);
    let status = match wait_for_child(&mut child, timeout) {
        Ok(status) => status,
        Err(error) => {
            let _ = reader.join();
            return Err(error);
        }
    };
    let stdout = join_stdout_thread(reader)?;
    if !status.success() {
        bail!("could not list installed Tesseract language packs");
    }
    let stdout = String::from_utf8(stdout).context("Tesseract returned non-UTF-8 language list")?;
    let languages: HashSet<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("List of available languages"))
        .map(str::to_string)
        .collect();
    *cached = Some(languages.clone());
    Ok(languages)
}

fn run_tesseract(image: Vec<u8>, languages: &str, deadline: Option<Instant>) -> Result<String> {
    let mut child = Command::new("tesseract")
        .args(["stdin", "stdout", "-l", languages, "tsv"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning local tesseract OCR")?;
    let timeout = match ocr_timeout(deadline) {
        Ok(timeout) => timeout,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let mut stdin = child.stdin.take().context("opening tesseract input")?;
    let writer = std::thread::spawn(move || stdin.write_all(&image));
    let stdout = child.stdout.take().context("opening tesseract output")?;
    let reader = read_stdout_thread(stdout);
    let status = match wait_for_child(&mut child, timeout) {
        Ok(status) => status,
        Err(error) => {
            let _ = writer.join();
            let _ = reader.join();
            return Err(error);
        }
    };
    writer.join()
        .map_err(|_| anyhow!("tesseract input worker stopped unexpectedly"))?
        .context("sending screenshot to local tesseract")?;
    let stdout = join_stdout_thread(reader)?;
    if !status.success() {
        bail!("local tesseract OCR failed; check that its traineddata language packs are installed");
    }
    String::from_utf8(stdout).context("tesseract returned non-UTF-8 TSV")
}

fn ocr_timeout(deadline: Option<Instant>) -> Result<Duration> {
    let timeout = deadline
        .map(|deadline| deadline.saturating_duration_since(Instant::now()))
        .unwrap_or(OCR_TIMEOUT)
        .min(OCR_TIMEOUT);
    if timeout.is_zero() {
        return Err(anyhow::Error::new(OcrTimedOut));
    }
    Ok(timeout)
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> Result<ExitStatus> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                let remaining = timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(anyhow::Error::new(OcrTimedOut));
                }
                std::thread::sleep(OCR_POLL_INTERVAL.min(remaining));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
    }
}

fn read_stdout_thread(stdout: ChildStdout) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut stdout = stdout;
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_stdout_thread(reader: std::thread::JoinHandle<std::io::Result<Vec<u8>>>) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| anyhow!("tesseract output worker stopped unexpectedly"))?
        .context("reading tesseract output")
}

fn parse_tesseract_tsv(tsv: &str, origin: (u32, u32)) -> Result<Vec<OcrLine>> {
    #[derive(Default)]
    struct Builder {
        text: Vec<String>,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
        confidence_sum: f64,
        confidence_count: u32,
    }

    let mut builders = Vec::<Builder>::new();
    let mut indexes = HashMap::<(u32, u32, u32, u32), usize>::new();
    for row in tsv.lines().skip(1) {
        let columns: Vec<&str> = row.splitn(12, '\t').collect();
        if columns.len() < 12 || columns[0] != "5" {
            continue;
        }
        let key = (
            columns[1].parse().context("invalid tesseract page number")?,
            columns[2].parse().context("invalid tesseract block number")?,
            columns[3].parse().context("invalid tesseract paragraph number")?,
            columns[4].parse().context("invalid tesseract line number")?,
        );
        let left: u32 = columns[6].parse().context("invalid tesseract word x coordinate")?;
        let top: u32 = columns[7].parse().context("invalid tesseract word y coordinate")?;
        let width: u32 = columns[8].parse().context("invalid tesseract word width")?;
        let height: u32 = columns[9].parse().context("invalid tesseract word height")?;
        let word = columns[11].trim();
        if word.is_empty() || width == 0 || height == 0 {
            continue;
        }
        let index = match indexes.get(&key) {
            Some(index) => *index,
            None => {
                let index = builders.len();
                indexes.insert(key, index);
                builders.push(Builder { x0: u32::MAX, y0: u32::MAX, ..Builder::default() });
                index
            }
        };
        let builder = &mut builders[index];
        builder.text.push(word.to_string());
        builder.x0 = builder.x0.min(left);
        builder.y0 = builder.y0.min(top);
        builder.x1 = builder.x1.max(left.saturating_add(width));
        builder.y1 = builder.y1.max(top.saturating_add(height));
        if let Ok(confidence) = columns[10].parse::<f64>() {
            if confidence >= 0.0 {
                builder.confidence_sum += confidence;
                builder.confidence_count += 1;
            }
        }
    }
    Ok(builders.into_iter().filter_map(|line| {
        if line.text.is_empty() || line.x0 == u32::MAX || line.y0 == u32::MAX {
            return None;
        }
        Some(OcrLine {
            text: line.text.join(" "),
            x: origin.0.saturating_add(line.x0),
            y: origin.1.saturating_add(line.y0),
            w: line.x1.saturating_sub(line.x0),
            h: line.y1.saturating_sub(line.y0),
            conf: if line.confidence_count == 0 { 0.0 } else { line.confidence_sum / f64::from(line.confidence_count) },
        })
    }).collect())
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
    use crate::network::GeoLookup;
    use crate::route::Health;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    /// A stand-in vinput that knows a US layout's letters and acknowledges everything else.
    fn fake_vinput(dir: &Path) -> PathBuf {
        let path = dir.join("vinput.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut writer = conn.try_clone().unwrap();
                    for line in BufReader::new(conn).lines().map_while(Result::ok) {
                        let reply = match line.strip_prefix("lookup ") {
                            Some("Shift_L") => "ok 42 0".to_string(),
                            Some(name) => match u32::from_str_radix(name.trim_start_matches('U'), 16).ok().and_then(char::from_u32) {
                                Some(c) if c.is_ascii_lowercase() => format!("ok {} 0", 16 + c as u32 - 'a' as u32),
                                _ => "err no key".to_string(),
                            },
                            None => "ok".to_string(),
                        };
                        if writeln!(writer, "{reply}").is_err() {
                            break;
                        }
                    }
                });
            }
        });
        path
    }

    fn make_handler(route_ok: bool) -> ToolHandler {
        let home = tempfile::tempdir().unwrap().keep();
        let local = LocalExecutor::with_input(fake_vinput(&home));
        build(&home, "name = \"shop\"\ncountry = \"DE\"", route_ok, local)
    }

    /// The container's real vinput and desktop.
    fn make_desktop_handler() -> ToolHandler {
        build(&tempfile::tempdir().unwrap().keep(), "name = \"desk\"\ncountry = \"US\"", true, LocalExecutor::new())
    }

    fn build(home: &Path, persona: &str, route_ok: bool, local: LocalExecutor) -> ToolHandler {
        let persona = Arc::new(Persona::parse(persona).unwrap());
        let route = Arc::new(Monitor::new(Arc::clone(&persona), None, GeoLookup::open(home)));
        route.set(health(route_ok, if route_ok { "ok" } else { "exit IP is in NL" }));
        let recorder = Arc::new(Recorder::open(home, "http://localhost:3456".into()).unwrap());
        ToolHandler::new(persona, route, recorder, local)
    }

    fn health(ok: bool, detail: &str) -> Health {
        Health { ok, route: "direct", checked_at: chrono::Utc::now(), exit: None, detail: detail.into() }
    }

    struct FixedDomain;

    impl PageTargets for FixedDomain {
        fn page_hosts(&self) -> Result<Vec<PageHost>> {
            Ok(vec![PageHost { host: "github.com".into(), scheme: PageScheme::Https }])
        }
    }

    fn test_secret_context(home: &Path, vault: Arc<Vault>) -> SecretContext {
        SecretContext {
            vault,
            audit: Arc::new(AuditLog::open(&home.join("audit.jsonl")).unwrap()),
            page_targets: Arc::new(FixedDomain),
        }
    }

    #[test]
    fn secret_placeholders_parse_without_expansion_and_bad_tags_refuse() {
        assert_eq!(parse_secret_text("ordinary text").unwrap(), None);
        assert_eq!(
            parse_secret_text("login: <secret>github</secret> + <secret>backup</secret>!").unwrap(),
            Some(vec![
                SecretPart::Text("login: ".into()),
                SecretPart::Secret("github".into()),
                SecretPart::Text(" + ".into()),
                SecretPart::Secret("backup".into()),
                SecretPart::Text("!".into()),
            ])
        );
        assert!(parse_secret_text("</secret>").is_err());
        assert!(parse_secret_text("<secret>github").is_err());
        assert!(parse_secret_text("<secret></secret>").is_err());
        assert!(parse_secret_text("<secret name=\"github\">x</secret>").is_err());
        assert!(parse_secret_text("<secret>github<secret>backup</secret>").is_err());
    }

    #[test]
    fn locked_missing_denied_and_empty_allowlist_refusals_are_audited_without_values() {
        let home = tempfile::tempdir().unwrap();
        let vault = Arc::new(Vault::open(home.path()));
        vault.init("passphrase").unwrap();
        vault.add_secret("github", b"hunter2-secret", vec!["github.com".into()], SecretType::Password).unwrap();
        vault.add_secret("empty", b"other-secret", vec![], SecretType::Password).unwrap();
        let ctx = test_secret_context(home.path(), Arc::clone(&vault));

        assert!(allowed_secret_metadata(&ctx, &["github"], "evil.example").is_err());
        assert!(allowed_secret_metadata(&ctx, &["github"], "login.github.com").is_ok());
        assert!(allowed_secret_metadata(&ctx, &["empty"], "github.com").is_err());
        assert!(allowed_secret_metadata(&ctx, &["missing"], "github.com").is_err());
        vault.lock();
        assert!(allowed_secret_metadata(&ctx, &["github"], "github.com").is_err());

        let audit = std::fs::read_to_string(home.path().join("audit.jsonl")).unwrap();
        assert!(audit.contains("domain_denied"));
        assert!(audit.contains("empty_allow_list"));
        assert!(audit.contains("secret_missing"));
        assert!(audit.contains("vault_locked"));
        assert!(!audit.contains("hunter2-secret"));
        assert!(!audit.contains("other-secret"));
    }

    #[test]
    fn totp_placeholder_resolves_to_helper_current_code() {
        let record = crate::vault::SecretRecord {
            name: "otp".into(),
            encrypted_value: b"GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".to_vec(),
            allowed_domains: vec!["github.com".into()],
            secret_type: SecretType::TotpSeed,
        };
        let now = std::time::UNIX_EPOCH + Duration::from_secs(59);
        assert_eq!(
            resolve_secret_value(&record, now).unwrap(),
            crate::totp::generate_totp("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ", now).unwrap()
        );
    }

    #[test]
    fn recordings_keep_placeholders_and_visual_suppression_is_sticky() {
        let handler = make_handler(true);
        assert!(!handler.handle(ToolCall::SessionStart, "agent").is_error);
        let refusal = handler.handle(ToolCall::Type {
            text: "Sign in <secret>github</secret>".into(),
            mode: "keys".into(),
            submit: false,
        }, "agent");
        assert!(refusal.is_error);
        let id = handler.recorder.current("agent").unwrap();
        let (events, _) = handler.recorder.events(&id, 1, 10).unwrap();
        let recorded = serde_json::to_string(&events).unwrap();
        assert!(recorded.contains("<secret>github</secret>"));
        assert!(!recorded.contains("hunter2-secret"));

        // Supply one synthetic prior frame, then assert MCP no longer sends it after taint.
        handler.recorder.record("agent", "screenshot", json!({}), None, 1, recording::Frame::Bytes(vec![1, 2, 3], "png"));
        handler.visual_sensitive.store(true, Ordering::SeqCst);
        let page = handler.handle(ToolCall::RecordingGet {
            id: Some(id), step: None, from_step: Some(1), limit: Some(10), frames: true,
        }, "agent");
        assert!(page.images.is_empty());
        let screenshot = handler.handle(ToolCall::Screenshot { frame_id: None, max_edge: None, region: None }, "agent");
        assert!(screenshot.error.unwrap().contains("persistent data volume"));

        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("run")).unwrap();
        let vault = Arc::new(Vault::open(home.path()));
        vault.init("passphrase").unwrap();
        let audit = Arc::new(AuditLog::open(&home.path().join("audit.jsonl")).unwrap());
        let first = build(
            home.path(),
            "name = \"shop\"\ncountry = \"DE\"",
            true,
            LocalExecutor::with_input(fake_vinput(home.path())),
        ).with_secret_context(Arc::clone(&vault), Arc::clone(&audit), Arc::new(FixedDomain), home.path()).unwrap();
        first.mark_visual_sensitive().unwrap();
        assert!(home.path().join("run/secret-visuals-disabled").exists());
        let restored = build(
            home.path(),
            "name = \"shop\"\ncountry = \"DE\"",
            true,
            LocalExecutor::new(),
        ).with_secret_context(vault, audit, Arc::new(FixedDomain), home.path()).unwrap();
        assert!(restored.visual_sensitive.load(Ordering::SeqCst));
    }

    #[test]
    fn action_without_session_rejected() {
        let handler = make_handler(true);
        let result = handler.handle(ToolCall::Screenshot { frame_id: None, max_edge: None, region: None }, "agent-1");
        assert!(result.error.unwrap().contains("session_start"));
    }

    #[test]
    fn one_session_at_a_time_on_the_one_persona() {
        let handler = make_handler(true);
        let started = handler.handle(ToolCall::SessionStart, "a");
        assert!(!started.is_error, "{:?}", started.error);
        assert_eq!(started.content["persona"], "shop");
        assert_eq!(handler.handle(ToolCall::SessionStart, "a").content["status"], "already_started");
        assert!(handler.handle(ToolCall::SessionStart, "b").error.unwrap().contains("held by 'a'"));
        assert!(handler.handle(ToolCall::SessionEnd, "b").is_error);
        assert!(handler.handle(ToolCall::ClipboardGet, "b").error.unwrap().contains("no active session"));

        let status = handler.handle(ToolCall::PersonaStatus, "b");
        assert_eq!(status.content["persona"], "shop");
        assert_eq!(status.content["declared"]["timezone"], "Europe/Berlin");
        assert_eq!(status.content["session"]["held_by_you"], false);
        assert!(status.content["route"]["ok"].as_bool().unwrap());

        assert!(!handler.handle(ToolCall::SessionEnd, "a").is_error);
        assert!(handler.handle(ToolCall::SessionEnd, "a").is_error, "already ended");
        assert!(!handler.handle(ToolCall::SessionStart, "b").is_error);
        let list = handler.handle(ToolCall::RecordingList { persona: None, limit: None }, "b");
        assert_eq!(list.content["recordings"][0]["persona"], "shop");
    }

    #[test]
    fn typing_uses_the_session_layout() {
        let handler = make_handler(true);
        handler.handle(ToolCall::SessionStart, "a");
        assert!(handler.local.can_type("hello").unwrap());
        assert!(!handler.local.can_type("Hello").unwrap(), "the fake layout has no capitals");
        let keys = handler.handle(ToolCall::Type { text: "Hi".into(), mode: "keys".into(), submit: false }, "a");
        assert!(keys.error.unwrap().contains("no key on this layout"));
    }

    #[test]
    fn failed_route_refuses_sessions_and_actions() {
        let handler = make_handler(false);
        let refused = handler.handle(ToolCall::SessionStart, "a");
        assert!(refused.error.unwrap().contains("exit IP is in NL"));
        let status = handler.handle(ToolCall::PersonaStatus, "a");
        assert_eq!(status.content["route"]["ok"], false, "status still answers");

        let handler = make_handler(true);
        handler.handle(ToolCall::SessionStart, "a");
        handler.route.set(health(false, "mid-session failure"));
        let refused = handler.handle(ToolCall::ClipboardSet { text: "x".into() }, "a");
        assert!(refused.error.unwrap().contains("mid-session failure"));
        assert!(!handler.handle(ToolCall::SessionEnd, "a").is_error, "ending is always allowed");
    }

    #[test]
    fn file_names_cannot_escape() {
        assert!(safe_file_name("report.pdf").is_ok());
        assert!(safe_file_name("../x").is_err());
        assert!(safe_file_name("a/b.txt").is_err());
        assert!(safe_file_name(".bashrc").is_err());
    }

    #[test]
    fn tesseract_tsv_groups_words_and_offsets_to_native_screen_coordinates() {
        let tsv = concat!(
            "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n",
            "5\t1\t1\t1\t1\t1\t10\t20\t30\t12\t90.5\tHello\n",
            "5\t1\t1\t1\t1\t2\t45\t20\t25\t12\t80.5\tworld\n",
            "5\t1\t1\t1\t2\t1\t10\t50\t20\t10\t70\tAgain\n",
        );
        let lines = parse_tesseract_tsv(tsv, (100, 200)).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "Hello world");
        assert_eq!((lines[0].x, lines[0].y, lines[0].w, lines[0].h), (110, 220, 60, 12));
        assert!((lines[0].conf - 85.5).abs() < 0.001);
        assert_eq!(lines[1].text, "Again");
    }

    #[test]
    fn visible_omnibox_host_must_match_an_https_cdp_candidate_with_confidence() {
        let line = |text: &str, conf| OcrLine {
            text: text.into(), x: 0, y: 0, w: 200, h: 30, conf,
        };
        let candidates = [
            PageHost { host: "github.com".into(), scheme: PageScheme::Https },
            PageHost { host: "docs.example".into(), scheme: PageScheme::Https },
        ];
        let only_github = &candidates[..1];
        assert_eq!(
            verify_visible_omnibox_lines(&[line("github.com/login", 94.0)], only_github),
            Ok("github.com".into()),
        );
        assert_eq!(
            verify_visible_omnibox_lines(&[line("https://github.com/login", 92.0)], only_github),
            Ok("github.com".into()),
        );
        assert_eq!(
            verify_visible_omnibox_lines(&[line("github.com/login", 94.0)], &candidates),
            Err(AddressBarRefusal::OtherSitesOpen),
            "an unloaded omnibox edit or a background tab must not vouch for the focused page",
        );
        assert_eq!(
            verify_visible_omnibox_lines(&[line("evil.example/login", 95.0)], &candidates),
            Err(AddressBarRefusal::HostMismatch),
        );
        assert_eq!(
            verify_visible_omnibox_lines(&[line("github.com", 69.9)], &candidates),
            Err(AddressBarRefusal::LowConfidence),
        );
        assert_eq!(
            verify_visible_omnibox_lines(&[line("http://github.com", 99.0)], &candidates),
            Err(AddressBarRefusal::InsecurePage),
        );
        assert_eq!(
            verify_visible_omnibox_lines(&[line("github.com", 99.0)], &[
                candidates[0].clone(),
                PageHost { host: "github.com".into(), scheme: PageScheme::Http },
            ]),
            Err(AddressBarRefusal::InsecurePage),
        );
        assert_eq!(
            verify_visible_omnibox_lines(&[], &candidates),
            Err(AddressBarRefusal::Unreadable),
        );
        assert_eq!(
            verify_visible_omnibox_lines(&[
                line("github.com", 99.0), line("profile", 99.0),
            ], &candidates),
            Err(AddressBarRefusal::Unreadable),
        );
        assert_eq!(
            verify_visible_omnibox_lines(&[line("docs.example/login", 99.0)], &candidates[1..]),
            Ok("docs.example".into()),
        );
        assert_eq!(
            verify_visible_omnibox_lines(&[line("github.com", 99.0)], &[]),
            Err(AddressBarRefusal::HostMismatch),
        );
    }

    #[test]
    fn ocr_coordinates_use_the_latest_view_scale_and_regions_map_to_native_pixels() {
        let view = View {
            id: Uuid::nil(),
            sx: 2.0 / 3.0,
            sy: 2.0 / 3.0,
            screen: Screen { width: 1920, height: 1080, scale: 1.0 },
        };
        let region = ocr_region_for_view(view, &ToolRegion { x: 100, y: 50, w: 200, h: 100 }).unwrap();
        assert_eq!(region, OcrRegion { x: 150, y: 75, w: 300, h: 150 });
        let json = ocr_line_json(&view, &OcrLine {
            text: "sample".into(), x: 150, y: 75, w: 300, h: 30, conf: 91.0,
        });
        assert_eq!(json["text"], "sample");
        assert_eq!((json["x"].as_u64(), json["y"].as_u64(), json["w"].as_u64(), json["h"].as_u64()),
            (Some(100), Some(50), Some(200), Some(20)));
        assert_eq!(json["conf"], 91.0);
        assert!(ocr_region_for_view(view, &ToolRegion { x: 1, y: 1, w: 0, h: 1 }).is_err());
    }

    #[test]
    fn ocr_and_zoom_regions_convert_physical_pixels_to_layout_coordinates_outward() {
        assert_eq!(
            layout_rect_for_physical(Rect { x: 125, y: 250, w: 126, h: 51 }, 1.25).unwrap(),
            Rect { x: 100, y: 200, w: 101, h: 41 },
        );
        assert!(layout_rect_for_physical(Rect { x: 0, y: 0, w: 1, h: 1 }, 0.0).is_err());

        let screen = Screen { width: 3840, height: 2160, scale: 2.0 };
        assert!((View::capture_scale(screen, 1280) - 2.0 / 3.0).abs() < 0.00001);
    }

    #[test]
    fn ocr_uses_persona_language_order_and_normalizes_search_text() {
        let installed: HashSet<String> = ["eng", "deu", "jpn", "chi_sim", "chi_tra", "heb"]
            .into_iter().map(str::to_string).collect();
        let languages = vec!["de-DE".into(), "de".into(), "en".into()];
        assert_eq!(select_tesseract_languages(&languages, &installed).unwrap(), "deu+eng");
        assert_eq!(select_tesseract_languages(&["ja".into(), "en".into()], &installed).unwrap(), "jpn+eng");
        assert_eq!(select_tesseract_languages(&["zh-TW".into()], &installed).unwrap(), "chi_tra");
        assert_eq!(select_tesseract_languages(&["zh-Hant".into()], &installed).unwrap(), "chi_tra");
        assert_eq!(select_tesseract_languages(&["zh-CN".into()], &installed).unwrap(), "chi_sim");
        assert_eq!(select_tesseract_languages(&["he-IL".into()], &installed).unwrap(), "heb");
        let english_only: HashSet<String> = ["eng"].into_iter().map(str::to_string).collect();
        assert!(select_tesseract_languages(&["ja".into()], &english_only).unwrap_err().to_string().contains("missing Tesseract pack"));
        assert!(select_tesseract_languages(&["zz".into()], &english_only).unwrap_err().to_string().contains("no Tesseract traineddata mapping"));
        assert!(select_tesseract_languages(&["en-US".into(), "ja".into()], &english_only).unwrap_err().to_string().contains("missing Tesseract pack"));
        assert!(select_tesseract_languages(&[], &installed).unwrap_err().to_string().contains("no OCR languages"));
        assert!(select_tesseract_languages(&["ja".into()], &HashSet::new()).is_err());
        assert_eq!(normalize_ocr_text("  Sign\tIN  Now "), "sign in now");
        assert!(normalize_ocr_text("Sign in Now").contains(&normalize_ocr_text("in now")));
    }

    #[test]
    fn ocr_cache_clear_discards_cached_text_and_frame_hash() {
        let mut cache = OcrCache::default();
        cache.insert(None, [7; 32], vec![OcrLine {
            text: "sensitive-looking text".into(), x: 0, y: 0, w: 1, h: 1, conf: 1.0,
        }]);
        cache.clear();
        assert!(cache.by_region.is_empty());
        assert!(cache.lru.is_empty());
    }

    #[test]
    fn ocr_cache_is_keyed_by_the_frame_hash() {
        let mut cache = OcrCache::default();
        let first_frame = image_fingerprint(b"P6 2 2 255\\ninitial sampled frame");
        let next_frame = image_fingerprint(b"P6 2 2 255\\nchanged sampled frame");
        cache.insert(None, first_frame, vec![OcrLine {
            text: "old OCR".into(), x: 0, y: 0, w: 1, h: 1, conf: 1.0,
        }]);
        assert_eq!(cache.get(None, first_frame).unwrap()[0].text, "old OCR");
        assert!(cache.get(None, next_frame).is_none());
        assert!(cache.by_region.is_empty());
    }

    #[test]
    fn ocr_cache_reuses_unchanged_regions_after_switching_crops() {
        let a = Some(OcrRegion { x: 0, y: 0, w: 10, h: 10 });
        let b = Some(OcrRegion { x: 10, y: 0, w: 10, h: 10 });
        let mut cache = OcrCache::default();
        cache.insert(a, [1; 32], vec![OcrLine {
            text: "region A".into(), x: 0, y: 0, w: 10, h: 10, conf: 90.0,
        }]);
        cache.insert(b, [2; 32], vec![OcrLine {
            text: "region B".into(), x: 10, y: 0, w: 10, h: 10, conf: 90.0,
        }]);
        assert_eq!(cache.get(a, [1; 32]).unwrap()[0].text, "region A");
        assert!(cache.get(a, [3; 32]).is_none(), "changed pixels in region A must invalidate it");
    }

    #[test]
    fn ocr_cache_is_bounded_to_the_most_recent_regions() {
        let mut cache = OcrCache::default();
        for x in 0..(OCR_CACHE_REGIONS as u32 + 1) {
            let region = Some(OcrRegion { x, y: 0, w: 1, h: 1 });
            cache.insert(region, [x as u8; 32], vec![]);
        }
        assert_eq!(cache.by_region.len(), OCR_CACHE_REGIONS);
        assert_eq!(cache.lru.len(), OCR_CACHE_REGIONS);
    }

    #[test]
    fn ocr_child_timeout_kills_and_reaps_process() {
        let mut child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let started = Instant::now();
        let error = wait_for_child(&mut child, Duration::from_millis(50)).unwrap_err();
        assert!(error.is::<OcrTimedOut>());
        assert!(child.try_wait().unwrap().is_some(), "timed-out child should have been reaped");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    #[ignore = "requires a Linux desktop with sway, grim, and vinput; run in Docker with cargo test -- --ignored"]
    fn session_then_act() {
        let handler = make_desktop_handler();
        assert!(!handler.handle(ToolCall::SessionStart, "agent-1").is_error);
        let result = handler.handle(ToolCall::Screenshot { frame_id: None, max_edge: None, region: None }, "agent-1");
        assert!(!result.is_error);
    }

    #[test]
    #[ignore = "requires a Linux desktop with sway, grim, and wf-recorder; run in Docker with cargo test -- --ignored"]
    fn session_is_recorded_from_start_to_end() {
        let handler = make_desktop_handler();
        assert!(handler.handle(ToolCall::RecordingGet {
            id: None, step: None, from_step: None, limit: None, frames: false,
        }, "a").is_error);

        handler.handle(ToolCall::SessionStart, "a");
        let again = handler.handle(ToolCall::SessionStart, "a");
        assert_eq!(again.content["status"], "already_started");
        handler.handle(ToolCall::Key { combo: "ctrl+l".into() }, "a");
        let share = handler.handle(ToolCall::RecordingShare { id: None, expires_in_s: None }, "a");
        assert!(share.content["url"].as_str().unwrap().contains("/recordings/"));
        handler.handle(ToolCall::SessionEnd, "a");

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
    #[ignore = "requires a Linux desktop with sway, grim, and vinput; run in Docker with cargo test -- --ignored"]
    fn computer_tool_dispatched() {
        let handler = make_desktop_handler();
        handler.handle(ToolCall::SessionStart, "agent-3");
        let result = handler.handle(
            ToolCall::Computer { action: "screenshot".into(), coordinate: None, text: None, extra: Default::default() },
            "agent-3",
        );
        assert!(!result.is_error);
    }
}
