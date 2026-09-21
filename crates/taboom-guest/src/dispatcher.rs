use crate::browser::BrowserSupervisor;
use crate::channel::ControlChannel;
use crate::emitter::Emitter;
use crate::input::{InputConfig, InputDevices, KeymapResolver, MockUinput, should_paste};
use crate::leak_guard::LeakGuard;
use crate::network::{self, NetworkSetup, Tun2socksConfig};
use crate::ocr::{self, TesseractBackend};
use crate::recorder::Recorder;
use crate::screen::{
    FrameManager, GrimCapture, ScreenCapture, ScreenError, TargetCheckResult,
    apply_byte_budget, check_target,
};
use crate::settle::{self, SettleDetector, SettleResult};
use crate::watchdog;
use anyhow::Result;
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::path::PathBuf;
use taboom_humanizer::{ActionPlan, HumanizerConfig, InputEvent};
use taboom_proto::*;
use tracing::{debug, info, warn};

pub struct InputManager {
    devices: InputDevices,
    humanizer: HumanizerConfig,
    rng: StdRng,
    keymap: KeymapResolver,
}

impl InputManager {
    pub fn new(seed: u64) -> Self {
        let backend: Box<dyn crate::input::UinputBackend> =
            match crate::input::LinuxUinput::create() {
                Ok(dev) => Box::new(dev),
                Err(_) => Box::new(MockUinput::new()),
            };
        let config = InputConfig::default();
        debug!(accel = ?config.accel_profile, speed = config.pointer_speed, "input config");
        Self {
            devices: InputDevices::new(backend, config),
            humanizer: HumanizerConfig::from_seed(seed),
            rng: StdRng::seed_from_u64(seed.wrapping_add(1)),
            keymap: KeymapResolver::new("us"),
        }
    }

    pub fn handle_mouse_move(&mut self, req: &MouseMoveReq) -> Message {
        let (cx, cy) = self.devices.cursor_position();
        let from = (cx as f64, cy as f64);
        let to = (req.x as f64, req.y as f64);

        let plan = taboom_humanizer::mouse::plan_move(
            from,
            to,
            20.0,
            &self.humanizer.style,
            &mut self.rng,
        );

        let result = self.emit_plan(plan);
        self.devices.set_cursor_position(req.x, req.y);
        result
    }

    pub fn handle_mouse_click(&mut self, req: &MouseClickReq) -> Message {
        let button = match req.button {
            MouseButton::Left => 0u8,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
        };
        let count = match req.click_type {
            ClickType::Single => 1,
            ClickType::Double => 2,
            ClickType::Triple => 3,
        };

        let (cx, cy) = self.devices.cursor_position();
        let needs_move = cx != req.x || cy != req.y;

        let mut full_plan = ActionPlan::new();

        if needs_move {
            let move_plan = taboom_humanizer::mouse::plan_move(
                (cx as f64, cy as f64),
                (req.x as f64, req.y as f64),
                20.0,
                &self.humanizer.style,
                &mut self.rng,
            );
            let offset = move_plan.duration_us();
            full_plan.extend(move_plan);

            let mut click_plan = taboom_humanizer::click::plan_click(
                button,
                count,
                &self.humanizer.style,
                &mut self.rng,
            );
            click_plan.offset(offset);
            full_plan.extend(click_plan);
        } else {
            full_plan = taboom_humanizer::click::plan_click(
                button,
                count,
                &self.humanizer.style,
                &mut self.rng,
            );
        }

        self.emit_plan(full_plan)
    }

    pub fn handle_key_type(&mut self, req: &KeyTypeReq) -> Message {
        if should_paste(&req.text, &self.keymap) {
            return self.handle_paste(&req.text);
        }

        let plan = taboom_humanizer::typing::plan_type(
            &req.text,
            &self.humanizer.style,
            &mut self.rng,
        );

        self.emit_plan(plan)
    }

    pub fn handle_key_press(&mut self, req: &KeyPressReq) -> Message {
        let mut plan = ActionPlan::new();
        let mut t_us: u64 = 0;

        for m in &req.modifiers {
            let code = match m {
                KeyModifier::Ctrl => 29u16,
                KeyModifier::Alt => 56,
                KeyModifier::Shift => 42,
                KeyModifier::Super => 125,
            };
            plan.push(t_us, InputEvent::Key { code, pressed: true });
            plan.push(t_us, InputEvent::Sync);
            t_us += 5000;
        }

        let actions = self.keymap.char_to_keycodes(
            req.key.chars().next().unwrap_or(' '),
        );
        for action in &actions {
            plan.push(t_us, InputEvent::Key { code: action.keycode, pressed: action.pressed });
            plan.push(t_us, InputEvent::Sync);
            t_us += 5000;
        }

        for m in req.modifiers.iter().rev() {
            let code = match m {
                KeyModifier::Ctrl => 29u16,
                KeyModifier::Alt => 56,
                KeyModifier::Shift => 42,
                KeyModifier::Super => 125,
            };
            plan.push(t_us, InputEvent::Key { code, pressed: false });
            plan.push(t_us, InputEvent::Sync);
            t_us += 5000;
        }

        self.emit_plan(plan)
    }

    pub fn handle_scroll(&mut self, req: &ScrollReq) -> Message {
        let direction = if req.delta_y.abs() >= req.delta_x.abs() {
            taboom_humanizer::scroll::ScrollDirection::Vertical
        } else {
            taboom_humanizer::scroll::ScrollDirection::Horizontal
        };
        let amount = if direction == taboom_humanizer::scroll::ScrollDirection::Vertical {
            req.delta_y
        } else {
            req.delta_x
        };

        let plan = taboom_humanizer::scroll::plan_scroll(
            direction,
            amount,
            &self.humanizer.style,
            &mut self.rng,
        );

        self.emit_plan(plan)
    }

    fn handle_paste(&mut self, _text: &str) -> Message {
        let mut plan = ActionPlan::new();
        plan.push(0, InputEvent::Key { code: 29, pressed: true });
        plan.push(0, InputEvent::Sync);
        plan.push(5000, InputEvent::Key { code: 47, pressed: true });
        plan.push(5000, InputEvent::Sync);
        plan.push(10000, InputEvent::Key { code: 47, pressed: false });
        plan.push(10000, InputEvent::Sync);
        plan.push(15000, InputEvent::Key { code: 29, pressed: false });
        plan.push(15000, InputEvent::Sync);
        self.emit_plan(plan)
    }

    pub fn emit_plan(&mut self, plan: ActionPlan) -> Message {
        match Emitter::execute_plan(&mut self.devices, &plan) {
            Ok(stats) => {
                debug!(
                    events = stats.events_emitted,
                    max_lateness_us = stats.max_lateness_us,
                    "input plan executed"
                );
                Message::CommandAck(CommandAck {
                    success: true,
                    error: None,
                })
            }
            Err(e) => {
                warn!("input plan failed: {e}");
                Message::CommandAck(CommandAck {
                    success: false,
                    error: Some(e.to_string()),
                })
            }
        }
    }

    pub fn release_all(&mut self) {
        if let Some(summary) = self.devices.drain_backend_events() {
            debug!(
                rels = summary.rels, keys = summary.keys,
                wheels = summary.wheels, syncs = summary.syncs,
                "drained mock backend events"
            );
        }
        if let Err(e) = self.devices.release_all() {
            warn!("release_all failed: {e}");
        }
    }
}

pub async fn run(channel: &mut ControlChannel) -> Result<()> {
    info!("dispatcher ready, waiting for commands");

    let mut browser = BrowserSupervisor::new(BrowserConfig::default());
    let mut input = InputManager::new(0);
    let mut frames = FrameManager::new(1920, 1080);
    let capture = GrimCapture;
    let settle_detector = SettleDetector::default();
    info!(
        threshold = settle_detector.threshold_pct,
        max_wait_ms = settle_detector.max_wait.as_millis() as u64,
        "settle detector initialized"
    );
    let mut recorder = Recorder::new(PathBuf::from("/tmp/taboom-recordings"), 7);
    let ocr_backend = TesseractBackend;
    let mut last_secret_value = None;
    let mut previous_frame_data: Vec<u8> = Vec::new();

    if let Ok(raw) = capture.capture_frame() {
        frames.update_geometry(raw.width, raw.height);
        debug!(w = raw.width, h = raw.height, size = raw.data.len(), "initial screen geometry");
    }

    tokio::spawn(async move {
        let mut bg_browser = BrowserSupervisor::new(BrowserConfig::default());
        bg_browser.ensure_installed().await.ok();
        info!(browser_state = ?bg_browser.state(), "browser checked");
        if let Some(version) = crate::browser::get_chrome_version().await {
            info!(version = %version, "Chrome version detected");
        }
        crate::browser::install_managed_policy().await.ok();
    });

    let proxy_config = Tun2socksConfig::default();
    info!(proxy_url = %proxy_config.proxy_url(), "proxy config ready");
    let proxy_args = proxy_config.build_args();
    debug!(args = ?proxy_args, "tun2socks args");

    let net_setup = NetworkSetup::for_proxy(taboom_proto::ProxyProtocol::Socks5);
    debug!(
        resolv = %net_setup.resolv_conf_content().trim(),
        tun = %net_setup.config.tun_device,
        "DNS config"
    );
    for (key, val) in NetworkSetup::sysctl_rules() {
        debug!(sysctl = %key, value = %val, "would apply sysctl");
    }
    for rule in NetworkSetup::iptables_drop_udp_rules() {
        debug!(rule = ?rule, "would apply iptables rule");
    }

    let route_report = network::current_route_state(true);
    info!(route = ?route_report.state, "initial route state");

    recorder.start().ok();
    info!(recorder = ?recorder.state(), session = %recorder.session_id, "recorder started");

    loop {
        let envelope = channel.recv().await?;
        debug!(id = %envelope.id, "received message");

        watchdog::notify_watchdog();

        match &envelope.body {
            Message::HeartbeatAck => {
                debug!("heartbeat acknowledged");
                browser.check_and_restart().await.ok();
            }
            Message::Screenshot(req) => {
                let reply = handle_screenshot(
                    req, &mut frames, &capture,
                    &mut previous_frame_data, &settle_detector,
                ).await;
                channel
                    .send(&Envelope::reply(envelope.id, reply))
                    .await?;

                if last_secret_value.is_some() {
                    run_leak_guard(&last_secret_value, &capture, &ocr_backend).await;
                }
            }
            Message::MouseMove(req) => {
                let reply = input.handle_mouse_move(req);
                channel
                    .send(&Envelope::reply(envelope.id, reply))
                    .await?;
            }
            Message::MouseClick(req) => {
                let fid = frames.current_frame_id();
                if let Err(ScreenError::OutOfBounds { .. }) =
                    frames.validate_coordinates(req.x, req.y, fid)
                {
                    warn!(x = req.x, y = req.y, "click coordinates out of bounds");
                }
                let reply = input.handle_mouse_click(req);
                channel
                    .send(&Envelope::reply(envelope.id, reply))
                    .await?;
            }
            Message::KeyType(req) => {
                let reply = input.handle_key_type(req);
                channel
                    .send(&Envelope::reply(envelope.id, reply))
                    .await?;
            }
            Message::KeyPress(req) => {
                let reply = input.handle_key_press(req);
                channel
                    .send(&Envelope::reply(envelope.id, reply))
                    .await?;
            }
            Message::Scroll(req) => {
                let reply = input.handle_scroll(req);
                channel
                    .send(&Envelope::reply(envelope.id, reply))
                    .await?;
            }
            Message::BrowserStart => {
                let reply = match browser.start().await {
                    Ok(()) => Message::BrowserStateReport(browser.state_report()),
                    Err(e) => {
                        warn!("browser start failed: {e}");
                        Message::CommandAck(CommandAck {
                            success: false,
                            error: Some(e.to_string()),
                        })
                    }
                };
                channel.send(&Envelope::reply(envelope.id, reply)).await?;
            }
            Message::BrowserStop => {
                let reply = match browser.stop().await {
                    Ok(()) => Message::BrowserStateReport(browser.state_report()),
                    Err(e) => {
                        warn!("browser stop failed: {e}");
                        Message::CommandAck(CommandAck {
                            success: false,
                            error: Some(e.to_string()),
                        })
                    }
                };
                channel.send(&Envelope::reply(envelope.id, reply)).await?;
            }
            Message::SecretTypeResponse(resp) => {
                let mut no_typo_style = input.humanizer.style.clone();
                no_typo_style.typo_rate = 0.0;
                let plan = taboom_humanizer::typing::plan_type(
                    &resp.value,
                    &no_typo_style,
                    &mut input.rng,
                );
                let reply = input.emit_plan(plan);
                last_secret_value = Some(resp.value.clone());
                channel
                    .send(&Envelope::reply(envelope.id, reply))
                    .await?;

                run_leak_guard(&last_secret_value, &capture, &ocr_backend).await;
            }
            Message::SecretRefused(refused) => {
                warn!(
                    reason = %refused.reason,
                    domain = ?refused.observed_domain,
                    "secret typing refused by host"
                );
            }
            Message::Shutdown => {
                info!("shutdown requested by host");
                input.release_all();
                browser.stop().await.ok();
                recorder.stop().ok();
                let removed = crate::recorder::apply_retention(
                    &PathBuf::from("/tmp/taboom-recordings"),
                    7,
                );
                if !removed.is_empty() {
                    info!(count = removed.len(), "removed expired recordings");
                }
                channel
                    .send(&Envelope::reply(envelope.id, Message::ShutdownAck))
                    .await?;
                break;
            }
            other => {
                debug!("unhandled message type: {other:?}");
            }
        }
    }

    Ok(())
}

async fn handle_screenshot(
    req: &ScreenshotReq,
    frames: &mut FrameManager,
    capture: &GrimCapture,
    previous_frame_data: &mut Vec<u8>,
    settle_detector: &SettleDetector,
) -> Message {
    if let Some(ref fid) = req.frame_id {
        let current = frames.current_frame_id();
        let result = check_target(0, 0, *fid, current, 0.1);
        if let TargetCheckResult::Changed { new_frame_id } = result {
            debug!(old = %fid, new = %new_frame_id, "frame changed since last screenshot");
        }
    }

    let png_data = match &req.region {
        Some(region) => capture.capture_region_to_png(region).await,
        None => capture.capture_to_png().await,
    };

    match png_data {
        Ok(data) => {
            if !previous_frame_data.is_empty() {
                let diff = settle::pixel_diff_pct(previous_frame_data, &data);
                let now = std::time::Instant::now();
                let result = settle_detector.check_settled(
                    &[diff],
                    &[now],
                );
                let settled = matches!(result, SettleResult::Settled { .. });
                debug!(
                    diff_pct = diff,
                    threshold = settle_detector.threshold_pct,
                    stable_ms = settle_detector.stable_duration.as_millis() as u64,
                    settled = settled,
                    "frame diff"
                );
            }
            *previous_frame_data = data.clone();

            let frame_id = frames.new_frame();
            let (native_w, native_h) = frames.native_size();
            let (final_data, format) = apply_byte_budget(&data, &req.format);

            Message::ScreenshotReply(ScreenshotReply {
                frame_id,
                native_width: native_w,
                native_height: native_h,
                returned_width: native_w,
                returned_height: native_h,
                scale: 1.0,
                format,
                data: final_data,
            })
        }
        Err(e) => {
            warn!("screenshot capture failed: {e}");
            Message::CommandAck(CommandAck {
                success: false,
                error: Some(e.to_string()),
            })
        }
    }
}

async fn run_leak_guard(
    last_secret: &Option<String>,
    capture: &GrimCapture,
    ocr_backend: &TesseractBackend,
) {
    let Some(secret_value) = last_secret else { return };

    let screenshot = capture.capture_to_png().await;
    let Ok(png_data) = screenshot else { return };

    let ocr_result = match ocr::perform_ocr(&png_data, ocr_backend) {
        Ok(result) => {
            let secrets = ocr::scan_for_secrets(
                &result.text_regions.iter().map(|r| r.text.as_str()).collect::<Vec<_>>().join(" "),
            );
            if !secrets.is_empty() {
                warn!(patterns = ?secrets, "potential secret patterns found on screen");
            }
            if let Some(ref url) = result.url {
                if let Some(ref domain) = result.domain {
                    debug!(url = %url, domain = %domain, "detected URL on screen");
                }
            }
            result
        }
        Err(e) => {
            debug!("OCR not available for leak guard: {e}");
            return;
        }
    };

    let leaks = LeakGuard::scan_for_leak(secret_value, &ocr_result);
    if !leaks.is_empty() {
        for leak in &leaks {
            warn!(
                x = leak.x, y = leak.y,
                w = leak.width, h = leak.height,
                "leaked secret at region"
            );
        }
        let redacted = LeakGuard::redact_image(&png_data, &leaks);
        debug!(original = png_data.len(), redacted = redacted.len(), "redacted screenshot");
    }
}
