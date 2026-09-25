use anyhow::{bail, Context, Result};
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicU8, Ordering};
use taboom_humanizer::idle::IdleMotion;
use taboom_humanizer::typing::{self, Keymap};
use taboom_humanizer::{click, drag, mouse, scroll, ActionPlan, HumanizerConfig, HumanizerStyle, InputEvent};
use tracing::debug;

/// A bounded local OCR screen command did not finish before its caller's deadline.
#[derive(Debug)]
pub(crate) struct OcrTimedOut;

impl std::fmt::Display for OcrTimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("local OCR exceeded its time limit")
    }
}

impl std::error::Error for OcrTimedOut {}

const LOCAL_COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Drives the container's desktop: screen via grim, input via the `vinput` daemon's one
/// persistent mouse and keyboard, timed by the humanizer. Clones share the device and style.
#[derive(Clone)]
pub struct LocalExecutor {
    env: Vec<(String, String)>,
    input_path: PathBuf,
    input: Arc<Mutex<Option<BufReader<UnixStream>>>>,
    human: Arc<Mutex<(HumanizerConfig, StdRng)>>,
    /// The session's layout, from vinput at session start; typing refuses to guess without it.
    keymap: Arc<Mutex<Option<Arc<Keymap>>>>,
    /// Held by every agent action; the idle hand only moves when it can take it.
    action: Arc<Mutex<()>>,
    idle: Arc<Mutex<IdleHand>>,
    buttons_held: Arc<AtomicU8>,
    trace_paused: Arc<std::sync::atomic::AtomicBool>,
}

/// The hand between actions: drifts and rests near where the last action left it.
struct IdleHand {
    enabled: bool,
    on_keyboard: bool,
    reset: bool,
    motion: IdleMotion,
    style: HumanizerStyle,
    rng: StdRng,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    /// Sway output scale. Mode dimensions are physical pixels; Grim regions use layout pixels.
    pub scale: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// A crop in Sway layout coordinates plus the active output scale grim should use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaledCrop {
    pub rect: Rect,
    pub scale: f64,
}

#[derive(Debug, Clone, Copy)]
struct LayoutRect {
    x: i64,
    y: i64,
    width: u64,
    height: u64,
}

#[derive(Clone, Copy)]
enum Axis {
    Vertical,
    Horizontal,
}

pub struct TracePauseGuard {
    local: LocalExecutor,
    was_enabled: bool,
    active: bool,
}

impl TracePauseGuard {
    pub fn resume(mut self) -> Result<()> {
        self.local.set_trace(self.was_enabled)?;
        self.active = false;
        Ok(())
    }
}

impl Drop for TracePauseGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = self.local.set_trace(self.was_enabled);
            self.active = false;
        }
    }
}

const BROWSER_START_TIMEOUT: Duration = Duration::from_secs(20);
/// Chrome's Wayland app_id, whatever its --user-data-dir (read from `swaymsg -t get_tree`).
const BROWSER_APP_ID: &str = "google-chrome";

impl LocalExecutor {
    pub fn new() -> Self {
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        Self::with_input(Path::new(&runtime).join("taboom-input.sock"))
    }

    /// Drives the vinput socket at `input_path`.
    pub fn with_input(input_path: PathBuf) -> Self {
        let env: Vec<(String, String)> = ["WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "SWAYSOCK"]
            .into_iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect();
        Self {
            env,
            input_path,
            input: Arc::new(Mutex::new(None)),
            human: Arc::new(Mutex::new((HumanizerConfig::from_seed(0), StdRng::seed_from_u64(0)))),
            keymap: Arc::new(Mutex::new(None)),
            action: Arc::new(Mutex::new(())),
            idle: Arc::new(Mutex::new(IdleHand {
                enabled: false,
                on_keyboard: false,
                reset: true,
                motion: IdleMotion::new(),
                style: HumanizerConfig::from_seed(0).style,
                rng: StdRng::seed_from_u64(1),
            })),
                buttons_held: Arc::new(AtomicU8::new(0)),
            trace_paused: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Runs one agent action with the hand to itself. `hand_on_keyboard`: Some(true) after
    /// typing, Some(false) after mouse work, None for actions that do not move the hand.
    pub fn exclusive<T>(&self, hand_on_keyboard: Option<bool>, f: impl FnOnce() -> T) -> T {
        let _hand = self.action.lock().unwrap();
        let out = f();
        if let Some(kb) = hand_on_keyboard {
            let mut idle = self.idle.lock().unwrap();
            idle.on_keyboard = kb;
            idle.reset = true;
        }
        out
    }

    /// Idle motion runs only while a persona session is active.
    pub fn set_idle(&self, on: bool) {
        let mut idle = self.idle.lock().unwrap();
        idle.enabled = on;
        idle.reset = true;
    }

    /// Background hand: between actions the cursor keeps resting and drifting like a person's,
    /// instead of freezing until the next command. Never moves while an action runs or a
    /// button is held.
    pub fn start_idle(&self) {
        let me = self.clone();
        std::thread::spawn(move || {
            let (mut ax, mut ay) = (0.0_f64, 0.0_f64);
            let mut last = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(8));
                let dt = last.elapsed().as_secs_f64() * 1000.0;
                last = Instant::now();
                let Ok(_hand) = me.action.try_lock() else { continue };
                if me.buttons_held.load(Ordering::Relaxed) != 0 {
                    continue;
                }
                let step = {
                    let mut guard = me.idle.lock().unwrap();
                    let idle = &mut *guard;
                    if !idle.enabled {
                        None
                    } else {
                        if idle.reset {
                            idle.motion.reset(idle.on_keyboard, &mut idle.rng);
                            idle.reset = false;
                            (ax, ay) = (0.0, 0.0);
                        }
                        Some(idle.motion.step(dt.min(50.0), idle.on_keyboard, &idle.style, &mut idle.rng))
                    }
                };
                let Some((dx, dy)) = step else { continue };
                ax += dx;
                ay += dy;
                let (ix, iy) = (ax.trunc(), ay.trunc());
                if ix != 0.0 || iy != 0.0 {
                    ax -= ix;
                    ay -= iy;
                    if let Err(e) = me.cmd(format!("rel {ix} {iy}")) {
                        debug!("idle move skipped: {e}");
                    }
                }
            }
        });
    }

    /// The persona's stable style (its "habits", from `humanizer.seed`), while every session
    /// draws fresh randomness so no two sessions move or type alike.
    pub fn set_seed(&self, seed: u64) {
        let config = HumanizerConfig::from_seed(seed);
        let mut idle = self.idle.lock().unwrap();
        idle.style = config.style.clone();
        idle.rng = StdRng::from_entropy();
        idle.reset = true;
        *self.human.lock().unwrap() = (config, StdRng::from_entropy());
    }

    fn run(&self, prog: &str, args: &[String]) -> Result<Vec<u8>> {
        debug!(prog, ?args, "local exec");
        let output = Command::new(prog)
            .args(args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .output()
            .with_context(|| format!("spawning {prog}"))?;
        if !output.status.success() {
            bail!("{prog} failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(output.stdout)
    }

    /// Runs a local screen command with an absolute deadline. Its pipes are drained concurrently
    /// so large PNG output cannot block the process while we poll for timeout.
    fn run_with_deadline(&self, prog: &str, args: &[String], deadline: Instant) -> Result<Vec<u8>> {
        if deadline <= Instant::now() {
            return Err(anyhow::Error::new(OcrTimedOut));
        }
        debug!(prog, ?args, "local exec with deadline");
        let mut child = Command::new(prog)
            .args(args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning {prog}"))?;
        let stdout = child.stdout.take().context("opening local command stdout")?;
        let stderr = child.stderr.take().context("opening local command stderr")?;
        let stdout_reader = drain_pipe_thread(stdout);
        let stderr_reader = drain_pipe_thread(stderr);
        let status = match wait_child_until(&mut child, deadline) {
            Ok(status) => status,
            Err(error) => return Err(error),
        };
        let stdout = read_pipe_until(stdout_reader, deadline)?;
        let stderr = read_pipe_until(stderr_reader, deadline)?;
        if !status.success() {
            bail!("{prog} failed: {}", String::from_utf8_lossy(&stderr).trim());
        }
        Ok(stdout)
    }

    /// Sends commands to vinput over one long-lived connection (held keys and buttons survive
    /// between calls; if taboomd dies, vinput releases everything). Reconnects once on failure.
    fn input(&self, lines: &[String]) -> Result<Vec<String>> {
        let mut guard = self.input.lock().unwrap();
        for attempt in 0..2 {
            if guard.is_none() {
                if self.trace_paused.load(Ordering::SeqCst) {
                    bail!("vinput disconnected during secret typing; refusing to reconnect with trace state unknown");
                }
                let stream = UnixStream::connect(&self.input_path)
                    .with_context(|| format!("connecting to vinput at {}", self.input_path.display()))?;
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                *guard = Some(BufReader::new(stream));
            }
            let conn = guard.as_mut().unwrap();
            match exchange(conn, lines) {
                Ok(replies) => return Ok(replies),
                Err(e) if attempt == 0 => {
                    debug!("vinput reconnect after: {e}");
                    *guard = None;
                    if self.trace_paused.load(Ordering::SeqCst) {
                        return Err(e);
                    }
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    fn cmd(&self, line: String) -> Result<String> {
        let reply = self.input(&[line])?.pop().unwrap_or_default();
        match reply.strip_prefix("ok") {
            Some(rest) => Ok(rest.trim().to_string()),
            None => bail!("{}", reply.trim_start_matches("err ").trim()),
        }
    }

    /// Replays a humanizer plan at its timestamps.
    fn play(&self, plan: &ActionPlan, axis: Axis) -> Result<()> {
        let started = Instant::now();
        let at = |i: usize| started + Duration::from_micros(plan.events[i].timestamp_us);
        let mut pending = (0, 0);
        for (i, ev) in plan.events.iter().enumerate() {
            if let Some(wait) = at(i).checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            let line = match ev.event {
                InputEvent::MouseRel { dx, dy } => {
                    pending = (pending.0 + dx, pending.1 + dy);
                    // Running late: fold overdue motion into one report, as a real mouse's next
                    // poll would, instead of bursting reports microseconds apart.
                    let next_move_due = plan.events[i + 1..]
                        .iter()
                        .position(|e| !matches!(e.event, InputEvent::Sync))
                        .map(|k| i + 1 + k)
                        .filter(|&k| matches!(plan.events[k].event, InputEvent::MouseRel { .. }))
                        .is_some_and(|k| at(k) <= Instant::now());
                    if next_move_due {
                        continue;
                    }
                    let (dx, dy) = std::mem::take(&mut pending);
                    format!("rel {dx} {dy}")
                }
                InputEvent::MouseButton { button, pressed } => format!("btn {button} {}", pressed as u8),
                InputEvent::Key { code, pressed } => format!("key {code} {}", pressed as u8),
                InputEvent::Wheel { delta, .. } => match axis {
                    Axis::Vertical => format!("wheel {delta} 0"),
                    Axis::Horizontal => format!("wheel 0 {delta}"),
                },
                InputEvent::Sync => continue,
            };
            self.cmd(line)?;
        }
        Ok(())
    }

    fn humanized<T>(&self, f: impl FnOnce(&HumanizerConfig, &mut StdRng) -> T) -> T {
        let mut h = self.human.lock().unwrap();
        let (cfg, rng) = &mut *h;
        f(cfg, rng)
    }

    /// Current size of the first active output. Read every time so a resize mid-session is seen.
    pub fn screen(&self) -> Result<Screen> {
        let out = self.run("swaymsg", &["-t".into(), "get_outputs".into(), "-r".into()])?;
        screen_from_outputs(&out)
    }

    /// Screen-size query for an OCR wait poll; propagates the poll's absolute deadline.
    pub fn screen_with_deadline(&self, deadline: Instant) -> Result<Screen> {
        let out = self.run_with_deadline("swaymsg", &["-t".into(), "get_outputs".into(), "-r".into()], deadline)?;
        screen_from_outputs(&out)
    }

    /// The first active output's mode (physical pixels) and scale, as the persona declares them.
    pub fn output(&self) -> Result<(u32, u32, f64)> {
        let out = self.run("swaymsg", &["-t".into(), "get_outputs".into(), "-r".into()])?;
        let outputs: serde_json::Value = serde_json::from_slice(&out).context("parsing sway outputs")?;
        let o = outputs
            .as_array()
            .and_then(|a| a.iter().find(|o| o["active"] == true))
            .context("no active sway output")?;
        let dim = |k: &str| o["current_mode"][k].as_u64().map(|v| v as u32);
        match (dim("width"), dim("height"), o["scale"].as_f64()) {
            (Some(w), Some(h), Some(scale)) if scale.is_finite() && scale > 0.0 => Ok((w, h, scale)),
            _ => bail!("sway output has no mode"),
        }
    }

    /// The screen or a region, scaled by `scale`. `format` is "png" or "ppm"
    /// (Debian's grim is built without JPEG; ppm is the cheap one for diffing).
    pub fn capture(&self, region: Option<Rect>, scale: f64, format: &str) -> Result<Vec<u8>> {
        self.capture_with_cursor(region, scale, format, true)
    }

    /// Captures a screen or region without the cursor. OCR and diffing use this so cursor motion
    /// neither obscures text nor invalidates image-based caches.
    pub fn capture_cursor_free(&self, region: Option<Rect>, scale: f64, format: &str) -> Result<Vec<u8>> {
        self.capture_with_cursor(region, scale, format, false)
    }

    /// Cursor-free grim capture for an OCR wait poll; cannot outlive its deadline.
    pub fn capture_cursor_free_with_deadline(
        &self,
        region: Option<Rect>,
        scale: f64,
        format: &str,
        deadline: Instant,
    ) -> Result<Vec<u8>> {
        self.run_with_deadline("grim", &grim_capture_args(region, scale, format, false), deadline)
    }

    fn capture_with_cursor(&self, region: Option<Rect>, scale: f64, format: &str, cursor: bool) -> Result<Vec<u8>> {
        self.run("grim", &grim_capture_args(region, scale, format, cursor))
    }

    /// Low-resolution cursor-free frame for cache invalidation, preserving 1/8 native pixels.
    pub fn diff_frame_at_scale(&self, output_scale: f64, deadline: Option<Instant>) -> Result<Vec<u8>> {
        let args = grim_capture_args(None, output_scale * 0.125, "ppm", false);
        match deadline {
            Some(deadline) => self.run_with_deadline("grim", &args, deadline),
            None => self.run("grim", &args),
        }
    }

    pub fn cursor(&self) -> Result<(u32, u32)> {
        let pos = self.cmd("pos".into())?;
        let mut it = pos.split_whitespace().filter_map(|v| v.parse().ok());
        match (it.next(), it.next()) {
            (Some(x), Some(y)) => Ok((x, y)),
            _ => bail!("vinput gave a bad position: {pos}"),
        }
    }

    /// Human-paced move along a Fitts/lognormal path, then an exact landing on the target.
    /// The hand lands within a few px of the aim, as a person aiming at a control does; returns
    /// where it landed.
    pub fn mouse_move(&self, x: u32, y: u32) -> Result<(u32, u32)> {
        let screen = self.screen()?;
        self.cmd(format!("size {} {}", screen.width, screen.height))?;
        let (fx, fy) = self.cursor()?;
        let (lx, ly) = self.landing(x, y, screen);
        let plan = self.humanized(|h, rng| {
            mouse::plan_move((fx as f64, fy as f64), (lx as f64, ly as f64), 24.0, &h.style, rng)
        });
        self.play(&plan, Axis::Vertical)?;
        // only corrects edge clamping; a normal plan already lands on the pixel
        if self.cursor()? != (lx, ly) {
            self.cmd(format!("abs {lx} {ly}"))?;
        }
        Ok((lx, ly))
    }

    fn landing(&self, x: u32, y: u32, screen: Screen) -> (u32, u32) {
        let (lx, ly) = self.humanized(|_, rng| mouse::landing_point((x as f64, y as f64), rng));
        (
            (lx.round().max(0.0) as u32).min(screen.width - 1),
            (ly.round().max(0.0) as u32).min(screen.height - 1),
        )
    }

    pub fn mouse_click(&self, x: u32, y: u32, button: &str, count: u32) -> Result<()> {
        let button = button_index(button)?;
        self.mouse_move(x, y)?;
        let plan = self.humanized(|h, rng| click::plan_click(button, count, &h.style, rng));
        self.play(&plan, Axis::Vertical)
    }

    pub fn mouse_button(&self, button: &str, down: bool) -> Result<()> {
        let index = button_index(button)?;
        self.cmd(format!("btn {index} {}", down as u8))?;
        let bit = 1u8 << index;
        if down {
            self.buttons_held.fetch_or(bit, Ordering::Relaxed);
        } else {
            self.buttons_held.fetch_and(!bit, Ordering::Relaxed);
        }
        Ok(())
    }

    pub fn drag(&self, from: (u32, u32), to: (u32, u32)) -> Result<()> {
        let start = self.mouse_move(from.0, from.1)?;
        let end = self.landing(to.0, to.1, self.screen()?);
        let plan = self.humanized(|h, rng| {
            drag::plan_drag((start.0 as f64, start.1 as f64), (end.0 as f64, end.1 as f64), &h.style, rng)
        });
        let played = self.play(&plan, Axis::Vertical);
        if played.is_err() {
            let _ = self.cmd("btn 0 0".into());
        }
        played?;
        // only corrects edge clamping, after the button is already up
        if self.cursor()? != end {
            self.cmd(format!("abs {} {}", end.0, end.1))?;
        }
        Ok(())
    }

    /// Wheel notches in bursts with reading pauses. Positive scrolls down / right.
    pub fn scroll(&self, x: u32, y: u32, delta_x: i32, delta_y: i32) -> Result<()> {
        self.mouse_move(x, y)?;
        for (axis, amount, dir) in [
            (Axis::Vertical, delta_y, scroll::ScrollDirection::Vertical),
            (Axis::Horizontal, delta_x, scroll::ScrollDirection::Horizontal),
        ] {
            if amount != 0 {
                let plan = self.humanized(|h, rng| scroll::plan_scroll(dir, amount, &h.style, rng));
                self.play(&plan, axis)?;
            }
        }
        Ok(())
    }

    /// Asks vinput which key and level types each character on the live layout, once per
    /// session: one batched round trip of ~200 lookups.
    pub fn load_keymap(&self) -> Result<()> {
        let names: Vec<String> = ["Shift_L".to_string(), "ISO_Level3_Shift".to_string()]
            .into_iter()
            .chain(typing::candidate_chars().map(typing::keysym_name))
            .collect();
        let lines: Vec<String> = names.iter().map(|n| format!("lookup {n}")).collect();
        let replies: std::collections::HashMap<&str, (u16, u8)> = names
            .iter()
            .zip(self.input(&lines)?)
            .filter_map(|(name, reply)| {
                let mut it = reply.strip_prefix("ok ")?.split_whitespace().map(str::parse::<u16>);
                Some((name.as_str(), (it.next()?.ok()?, it.next()?.ok()? as u8)))
            })
            .collect();
        let keymap = Keymap::build(|name| replies.get(name).copied())
            .context("vinput found no Shift key; is it running a keymap?")?;
        *self.keymap.lock().unwrap() = Some(Arc::new(keymap));
        Ok(())
    }

    fn keymap(&self) -> Result<Arc<Keymap>> {
        self.keymap.lock().unwrap().clone().context("no keyboard layout loaded; call session_start first")
    }

    /// Real key presses with human timing and occasional corrected typos. The caller pastes
    /// text this layout cannot type (see `can_type`).
    pub fn key_type(&self, text: &str) -> Result<()> {
        let keymap = self.keymap()?;
        if !typing::can_type(text, &keymap) {
            bail!("text has characters with no key on this layout; use paste mode");
        }
        let plan = self.humanized(|h, rng| typing::plan_type(text, &keymap, &h.style, rng));
        let played = self.play(&plan, Axis::Vertical);
        if played.is_err() {
            let _ = self.cmd("release".into());
        }
        played
    }

    /// Performs a caller-supplied final authorization check after validating the layout and
    /// XKB state, then types exact layout-aware strokes without paste or typo planning.
    pub fn key_type_exact_checked(
        &self,
        text: &str,
        before_first_key: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let keymap = self.keymap()?;
        if !typing::can_type(text, &keymap) {
            bail!("secret contains a character with no key on this layout");
        }
        let state = self.cmd("state".into())?;
        if !keyboard_state_is_neutral(&state) {
            bail!("secret typing requires neutral depressed, latched, and locked modifiers and keyboard layout group");
        }
        before_first_key()?;
        let (shift_code, altgr_code) = keymap.modifier_codes();
        let result = (|| {
            for ch in text.chars() {
                let stroke = keymap.stroke(ch).context("secret character has no key on this layout")?;
                let mut modifiers = Vec::with_capacity(2);
                if stroke.shift {
                    modifiers.push(shift_code);
                }
                if stroke.altgr {
                    let code = altgr_code.context("layout requires unavailable AltGr")?;
                    if !modifiers.contains(&code) {
                        modifiers.push(code);
                    }
                }
                for code in &modifiers {
                    self.cmd(format!("key {code} 1"))?;
                }
                if let Err(error) = self.cmd(format!("key {} 1", stroke.code)) {
                    for code in modifiers.iter().rev() {
                        let _ = self.cmd(format!("key {code} 0"));
                    }
                    return Err(error);
                }
                std::thread::sleep(Duration::from_millis(45));
                let release = self.cmd(format!("key {} 0", stroke.code));
                for code in modifiers.iter().rev() {
                    let _ = self.cmd(format!("key {code} 0"));
                }
                release?;
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = self.cmd("release".into());
        }
        result
    }

    /// Stops trace output for a sensitive key sequence. The guard restores the previous state
    /// if the sequence returns early or unwinds.
    pub fn pause_trace(&self) -> Result<TracePauseGuard> {
        let was_enabled = self.cmd("trace 0".into())? == "1";
        self.trace_paused.store(true, Ordering::SeqCst);
        Ok(TracePauseGuard { local: self.clone(), was_enabled, active: true })
    }

    fn set_trace(&self, enabled: bool) -> Result<()> {
        let result = self.cmd(format!("trace {}", enabled as u8)).map(|_| ());
        self.trace_paused.store(false, Ordering::SeqCst);
        result
    }

    pub fn can_type(&self, text: &str) -> Result<bool> {
        Ok(typing::can_type(text, &*self.keymap()?))
    }

    /// xdotool-style keys: "ctrl+shift+t", "Return", or a space-separated sequence "ctrl+a Delete".
    pub fn key_press(&self, keys: &str) -> Result<()> {
        for combo in keys.split_whitespace() {
            let syms = combo_keysyms(combo);
            if syms.is_empty() {
                continue;
            }
            let result = (|| {
                for s in &syms {
                    self.cmd(format!("sym {s} 1"))?;
                    self.gap(25.0, 10.0);
                }
                self.gap(60.0, 15.0);
                for s in syms.iter().rev() {
                    self.cmd(format!("sym {s} 0"))?;
                    self.gap(20.0, 8.0);
                }
                anyhow::Ok(())
            })();
            if result.is_err() {
                let _ = self.cmd("release".into());
            }
            result?;
            self.gap(90.0, 30.0);
        }
        Ok(())
    }

    pub fn key_state(&self, key: &str, down: bool) -> Result<()> {
        self.cmd(format!("sym {} {}", keysym(key), down as u8)).map(drop)
    }

    pub fn release_all(&self) -> Result<()> {
        self.buttons_held.store(0, Ordering::Relaxed);
        self.cmd("release".into()).map(drop)
    }

    fn gap(&self, mean_ms: f64, sd_ms: f64) {
        let ms = self.humanized(|_, rng| taboom_humanizer::lognormal_sample(rng, mean_ms, sd_ms));
        std::thread::sleep(Duration::from_secs_f64(ms.clamp(1.0, 2000.0) / 1000.0));
    }

    pub fn clipboard_set(&self, text: &str) -> Result<()> {
        // wl-copy forks a server that owns the selection; any piped stdout/stderr would stay
        // open in that child and block us forever, so only stdin is piped.
        let mut child = Command::new("wl-copy")
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawning wl-copy")?;
        child.stdin.take().context("wl-copy stdin")?.write_all(text.as_bytes())?;
        let status = child.wait()?;
        if !status.success() {
            bail!("wl-copy exited with {status}");
        }
        Ok(())
    }

    pub fn clipboard_get(&self) -> Result<String> {
        match self.run("wl-paste", &["--no-newline".into()]) {
            Ok(out) => Ok(String::from_utf8_lossy(&out).into_owned()),
            Err(e) if e.to_string().contains("No selection") => Ok(String::new()),
            Err(e) => Err(e),
        }
    }

    /// Text in: typed when the layout has every key, otherwise pasted like a person would.
    pub fn enter_text(&self, text: &str) -> Result<()> {
        if self.can_type(text)? {
            self.key_type(text)
        } else {
            self.clipboard_set(text)?;
            self.key_press("ctrl+v")
        }
    }

    /// app_id of the focused window, if any.
    pub fn focused_app(&self) -> Result<Option<String>> {
        let out = self.run("swaymsg", &["-t".into(), "get_tree".into(), "-r".into()])?;
        let tree: serde_json::Value = serde_json::from_slice(&out).context("parsing sway tree")?;
        Ok(find_focused(&tree).and_then(|n| n["app_id"].as_str().map(str::to_string)))
    }

    /// The secret path only types into a focused Chrome window, and only when that window is
    /// not in fullscreen mode.
    pub fn focused_chrome_is_fullscreen(&self) -> Result<bool> {
        let out = self.run("swaymsg", &["-t".into(), "get_tree".into(), "-r".into()])?;
        let tree: serde_json::Value = serde_json::from_slice(&out).context("parsing sway tree")?;
        let (node, fullscreen) = find_focused_with_fullscreen(&tree, false)
            .context("there is no focused window")?;
        if node["app_id"].as_str() != Some(BROWSER_APP_ID) {
            bail!("Chrome is not the focused window");
        }
        Ok(fullscreen)
    }

    /// Finds a small crop containing Chrome's omnibox text. The crop is derived only from the
    /// focused Chrome window and the active output's Sway geometry; it is never inferred from a
    /// whole-screen OCR result. Chrome's desktop UI keeps the tab strip above this toolbar band.
    pub fn focused_chrome_omnibox_crop(&self) -> Result<ScaledCrop> {
        let tree_out = self.run("swaymsg", &["-t".into(), "get_tree".into(), "-r".into()])?;
        let tree: serde_json::Value = serde_json::from_slice(&tree_out).context("parsing sway tree")?;
        let focused = find_focused(&tree).context("there is no focused window")?;
        if focused["app_id"].as_str() != Some(BROWSER_APP_ID) {
            bail!("Chrome is not the focused window");
        }
        let window = layout_rect(&focused["rect"]).context("focused Chrome window has no usable geometry")?;

        let outputs_out = self.run("swaymsg", &["-t".into(), "get_outputs".into(), "-r".into()])?;
        let outputs: serde_json::Value = serde_json::from_slice(&outputs_out).context("parsing sway outputs")?;
        let active: Vec<&serde_json::Value> = outputs
            .as_array()
            .context("sway outputs are not an array")?
            .iter()
            .filter(|output| output["active"] == true)
            .collect();
        let [output] = active.as_slice() else {
            bail!("omnibox OCR requires one active output with unambiguous scale");
        };
        let output_rect = layout_rect(&output["rect"]).context("active output has no usable geometry")?;
        let scale = output["scale"].as_f64().context("active output has no scale")?;
        omnibox_crop_from_geometry(window, output_rect, scale)
            .context("could not derive a safe Chrome omnibox crop")
    }

    /// Brings the browser forward the way a person would: the desktop's super+b shortcut, which
    /// focuses an open window or starts Chrome.
    pub fn focus_browser(&self) -> Result<()> {
        if self.focused_app()?.as_deref() == Some(BROWSER_APP_ID) {
            return Ok(());
        }
        self.key_press("super+b")?;
        let started = Instant::now();
        while started.elapsed() < BROWSER_START_TIMEOUT {
            if self.focused_app()?.as_deref() == Some(BROWSER_APP_ID) {
                std::thread::sleep(Duration::from_millis(300));
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        bail!("the browser did not come up within {}s", BROWSER_START_TIMEOUT.as_secs())
    }

    /// New tab, type the address, Enter. Keyboard only.
    pub fn open_url(&self, url: &str) -> Result<()> {
        self.focus_browser()?;
        self.key_press("ctrl+t")?;
        std::thread::sleep(Duration::from_millis(350));
        self.enter_text(url)?;
        self.key_press("Return")
    }

    /// Screen video into `path` (.mkv, stays playable if we crash). Scaled to at most 1280 wide.
    pub fn start_video(&self, path: &Path) -> Result<Child> {
        let screen = self.screen()?;
        let mut args: Vec<String> = vec![
            "-f".into(), path.display().to_string(),
            "-c".into(), "libx264".into(),
            "-x".into(), "yuv420p".into(),
            "-p".into(), "preset=veryfast".into(),
            "-p".into(), "crf=28".into(),
        ];
        if screen.width > 1280 {
            args.extend(["-F".into(), "scale=1280:-2".into()]);
        }
        Command::new("wf-recorder")
            .args(&args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawning wf-recorder")
    }
}

/// Stops a recording cleanly (SIGINT finalizes the file), then remuxes it to a seekable mp4.
pub fn finish_video(mut child: Child, mkv: &Path) -> Result<PathBuf> {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    let _ = kill(Pid::from_raw(child.id() as i32), Signal::SIGINT);
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait()?.is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.wait();
    let mp4 = mkv.with_extension("mp4");
    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-i"])
        .arg(mkv)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(&mp4)
        .status()
        .context("spawning ffmpeg")?;
    if !status.success() {
        bail!("ffmpeg could not remux {}; the .mkv is kept", mkv.display());
    }
    let _ = std::fs::remove_file(mkv);
    Ok(mp4)
}

fn exchange(conn: &mut BufReader<UnixStream>, lines: &[String]) -> Result<Vec<String>> {
    let mut out = String::new();
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    conn.get_mut().write_all(out.as_bytes())?;
    let mut replies = Vec::with_capacity(lines.len());
    for _ in lines {
        let mut reply = String::new();
        if conn.read_line(&mut reply)? == 0 {
            bail!("vinput closed the connection");
        }
        replies.push(reply.trim_end().to_string());
    }
    Ok(replies)
}

fn find_focused(node: &serde_json::Value) -> Option<&serde_json::Value> {
    if node["focused"] == true {
        return Some(node);
    }
    ["nodes", "floating_nodes"]
        .iter()
        .filter_map(|k| node[*k].as_array())
        .flatten()
        .find_map(find_focused)
}

fn layout_rect(value: &serde_json::Value) -> Option<LayoutRect> {
    Some(LayoutRect {
        x: value["x"].as_i64()?,
        y: value["y"].as_i64()?,
        width: value["width"].as_u64()?,
        height: value["height"].as_u64()?,
    })
}

/// Locates the URL text area in Chrome's toolbar, expressed in Sway layout coordinates. We crop
/// below the tabs/title row and keep the horizontal band inside the omnibox between its leading
/// navigation controls and trailing toolbar controls. A crop that would touch page content or
/// fall outside the one active output is rejected.
fn omnibox_crop_from_geometry(
    window: LayoutRect,
    output: LayoutRect,
    scale: f64,
) -> Option<ScaledCrop> {
    // Chrome's desktop tab strip is about 36-40 layout pixels tall. Start below that row and
    // keep the OCR band around the URL text baseline, away from tab titles and page content.
    const URL_TEXT_BAND_TOP: i64 = 48;
    const URL_TEXT_BAND_HEIGHT: i64 = 28;
    const OMNIBOX_LEFT_INSET: i64 = 120;
    const OMNIBOX_RIGHT_INSET: i64 = 120;
    const MIN_OMNIBOX_WIDTH: i64 = 160;

    if !scale.is_finite() || !(0.5..=8.0).contains(&scale)
        || window.width == 0 || window.height == 0
        || output.width == 0 || output.height == 0
    {
        return None;
    }
    let (wx, wy, ww, wh) = (window.x, window.y, i64::try_from(window.width).ok()?, i64::try_from(window.height).ok()?);
    let (ox, oy, ow, oh) = (output.x, output.y, i64::try_from(output.width).ok()?, i64::try_from(output.height).ok()?);
    let (wr, wb, or_, ob) = (wx.checked_add(ww)?, wy.checked_add(wh)?, ox.checked_add(ow)?, oy.checked_add(oh)?);
    // Partial/off-output Chrome geometry is ambiguous: do not try to crop through another output.
    if wx < ox || wy < oy || wr > or_ || wb > ob {
        return None;
    }

    let x = wx.checked_add(OMNIBOX_LEFT_INSET)?;
    let right = wr.checked_sub(OMNIBOX_RIGHT_INSET)?;
    let y = wy.checked_add(URL_TEXT_BAND_TOP)?;
    let bottom = y.checked_add(URL_TEXT_BAND_HEIGHT)?;
    if right.checked_sub(x)? < MIN_OMNIBOX_WIDTH || bottom > wb || y < oy || bottom > ob {
        return None;
    }
    Some(ScaledCrop {
        rect: Rect {
            x: u32::try_from(x).ok()?,
            y: u32::try_from(y).ok()?,
            w: u32::try_from(right.checked_sub(x)?).ok()?,
            h: u32::try_from(URL_TEXT_BAND_HEIGHT).ok()?,
        },
        scale,
    })
}

fn find_focused_with_fullscreen(
    node: &serde_json::Value,
    inherited_fullscreen: bool,
) -> Option<(&serde_json::Value, bool)> {
    let fullscreen = inherited_fullscreen || node["fullscreen_mode"].as_i64().unwrap_or(0) != 0;
    if node["focused"] == true {
        return Some((node, fullscreen));
    }
    ["nodes", "floating_nodes"]
        .iter()
        .filter_map(|k| node[*k].as_array())
        .flatten()
        .find_map(|child| find_focused_with_fullscreen(child, fullscreen))
}

fn keyboard_state_is_neutral(state: &str) -> bool {
    let mut fields = state.split_whitespace();
    (0..5).all(|_| fields.next().and_then(|v| v.parse::<u32>().ok()) == Some(0))
        && fields.next().is_none()
}

fn button_index(button: &str) -> Result<u8> {
    match button {
        "left" => Ok(click::BTN_LEFT),
        "right" => Ok(click::BTN_RIGHT),
        "middle" => Ok(click::BTN_MIDDLE),
        other => bail!("button must be left, right or middle, not {other:?}"),
    }
}

/// "ctrl+shift+t" -> [Control_L, Shift_L, t]
fn combo_keysyms(combo: &str) -> Vec<String> {
    combo.split('+').filter(|p| !p.is_empty()).map(keysym).collect()
}

/// Common names (ctrl, enter, pagedown, cmd, F5, a) to XKB keysym names; others pass through.
fn keysym(k: &str) -> String {
    match k.to_lowercase().as_str() {
        "ctrl" | "control" => "Control_L".into(),
        "shift" => "Shift_L".into(),
        "alt" | "option" => "Alt_L".into(),
        "super" | "meta" | "win" | "cmd" | "logo" => "Super_L".into(),
        "altgr" => "ISO_Level3_Shift".into(),
        "enter" | "return" => "Return".into(),
        "tab" => "Tab".into(),
        "escape" | "esc" => "Escape".into(),
        "backspace" => "BackSpace".into(),
        "delete" | "del" => "Delete".into(),
        "insert" | "ins" => "Insert".into(),
        "space" => "space".into(),
        "up" => "Up".into(),
        "down" => "Down".into(),
        "left" => "Left".into(),
        "right" => "Right".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "pageup" | "page_up" | "prior" => "Prior".into(),
        "pagedown" | "page_down" | "next" => "Next".into(),
        "plus" | "+" => "plus".into(),
        "minus" | "-" => "minus".into(),
        other if other.len() > 1 && other.starts_with('f') && other[1..].parse::<u32>().is_ok() => {
            other.to_uppercase()
        }
        _ if k.chars().count() == 1 => k.to_lowercase(),
        _ => k.to_string(),
    }
}

/// Width and height from a PNG's IHDR chunk.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]);
    Some((be(16), be(20)))
}

fn screen_from_outputs(out: &[u8]) -> Result<Screen> {
    let outputs: serde_json::Value = serde_json::from_slice(out).context("parsing sway outputs")?;
    let o = outputs
        .as_array()
        .and_then(|a| a.iter().find(|o| o["active"] == true))
        .context("no active sway output")?;
    let dim = |k: &str| o["current_mode"][k].as_u64().map(|v| v as u32);
    match (dim("width"), dim("height"), o["scale"].as_f64()) {
        (Some(width), Some(height), Some(scale)) if scale.is_finite() && scale > 0.0 => Ok(Screen { width, height, scale }),
        _ => bail!("sway output has no mode"),
    }
}

fn grim_capture_args(region: Option<Rect>, scale: f64, format: &str, cursor: bool) -> Vec<String> {
    let mut args = vec!["-s".into(), format!("{scale:.4}")];
    if cursor {
        args.push("-c".into());
    }
    if let Some(region) = region {
        args.extend([
            "-g".into(),
            format!("{},{} {}x{}", region.x, region.y, region.w, region.h),
        ]);
    }
    args.extend(["-t".into(), format.into(), "-".into()]);
    args
}

fn drain_pipe_thread<R: Read + Send + 'static>(mut pipe: R) -> Receiver<std::io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = pipe.read_to_end(&mut bytes).map(|_| bytes);
        let _ = sender.send(result);
    });
    receiver
}

fn read_pipe_until(reader: Receiver<std::io::Result<Vec<u8>>>, deadline: Instant) -> Result<Vec<u8>> {
    match reader.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(result) => result.context("reading local command output"),
        Err(RecvTimeoutError::Timeout) => Err(anyhow::Error::new(OcrTimedOut)),
        Err(RecvTimeoutError::Disconnected) => bail!("local command pipe reader stopped unexpectedly"),
    }
}

fn wait_child_until(child: &mut Child, deadline: Instant) -> Result<ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(anyhow::Error::new(OcrTimedOut));
                }
                std::thread::sleep(LOCAL_COMMAND_POLL_INTERVAL.min(remaining));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sway_screen_dimensions_keep_the_active_output_scale() {
        let outputs = br#"[{"active":true,"current_mode":{"width":3840,"height":2160},"scale":2.0}]"#;
        assert_eq!(screen_from_outputs(outputs).unwrap(), Screen { width: 3840, height: 2160, scale: 2.0 });
        let bad_scale = br#"[{"active":true,"current_mode":{"width":3840,"height":2160},"scale":0.0}]"#;
        assert!(screen_from_outputs(bad_scale).is_err());
    }

    #[test]
    fn grim_region_uses_layout_coordinates_and_requested_scale() {
        assert_eq!(
            grim_capture_args(Some(Rect { x: 100, y: 200, w: 101, h: 41 }), 1.25, "png", false),
            ["-s", "1.2500", "-g", "100,200 101x41", "-t", "png", "-"],
        );
        assert_eq!(grim_capture_args(None, 0.25, "ppm", false)[1], "0.2500");
    }

    #[test]
    fn combos_map_to_keysyms() {
        assert_eq!(combo_keysyms("ctrl+shift+T"), ["Control_L", "Shift_L", "t"]);
        assert_eq!(combo_keysyms("Enter"), ["Return"]);
        assert_eq!(combo_keysyms("cmd+l"), ["Super_L", "l"]);
        assert_eq!(combo_keysyms("F5"), ["F5"]);
        assert_eq!(combo_keysyms("KP_Enter"), ["KP_Enter"]);
        assert_eq!(combo_keysyms("super+Shift+q"), ["Super_L", "Shift_L", "q"]);
    }

    #[test]
    fn png_header_size() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(1280u32.to_be_bytes());
        png.extend(720u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((1280, 720)));
        assert_eq!(png_size(b"nope"), None);
    }

    #[test]
    fn deadline_command_kills_and_reaps_a_slow_child() {
        let local = LocalExecutor::new();
        let started = Instant::now();
        let error = local
            .run_with_deadline(
                "/bin/sleep",
                &["5".into()],
                started + Duration::from_millis(60),
            )
            .unwrap_err();
        assert!(error.is::<OcrTimedOut>());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn focused_node_found_in_nested_tree() {
        let tree = serde_json::json!({
            "focused": false,
            "nodes": [{ "focused": false, "nodes": [], "floating_nodes": [
                { "focused": true, "app_id": "google-chrome" }
            ]}]
        });
        assert_eq!(find_focused(&tree).unwrap()["app_id"], "google-chrome");
    }

    #[test]
    fn fullscreen_state_inherits_from_focused_window_ancestors() {
        let tree = serde_json::json!({
            "fullscreen_mode": 1,
            "focused": false,
            "nodes": [{ "focused": false, "nodes": [], "floating_nodes": [
                { "focused": true, "app_id": "google-chrome", "fullscreen_mode": 0 }
            ]}]
        });
        let (node, fullscreen) = find_focused_with_fullscreen(&tree, false).unwrap();
        assert_eq!(node["app_id"], "google-chrome");
        assert!(fullscreen);
    }

    #[test]
    fn omnibox_crop_uses_focused_window_bounds_and_output_scale() {
        let crop = omnibox_crop_from_geometry(
            LayoutRect { x: 20, y: 30, width: 1200, height: 800 },
            LayoutRect { x: 0, y: 0, width: 1920, height: 1080 },
            2.0,
        ).unwrap();
        assert_eq!(crop.rect.x, 140);
        assert_eq!(crop.rect.y, 78);
        assert_eq!(crop.rect.w, 960);
        assert_eq!(crop.rect.h, 28);
        assert_eq!(crop.scale, 2.0);
    }

    #[test]
    fn omnibox_crop_refuses_ambiguous_or_out_of_bounds_geometry() {
        let output = LayoutRect { x: 0, y: 0, width: 1920, height: 1080 };
        assert!(omnibox_crop_from_geometry(
            LayoutRect { x: 100, y: 1000, width: 1000, height: 200 }, output, 1.0,
        ).is_none());
        assert!(omnibox_crop_from_geometry(
            LayoutRect { x: 100, y: 100, width: 200, height: 400 }, output, 1.0,
        ).is_none());
        assert!(omnibox_crop_from_geometry(
            LayoutRect { x: -1, y: 100, width: 1000, height: 400 }, output, 1.0,
        ).is_none());
        assert!(omnibox_crop_from_geometry(
            LayoutRect { x: 100, y: 100, width: 1000, height: 400 }, output, f64::NAN,
        ).is_none());
    }

    #[test]
    fn exact_typing_requires_neutral_keyboard_and_layout_state() {
        assert!(keyboard_state_is_neutral("0 0 0 0 0"));
        assert!(!keyboard_state_is_neutral("2 0 0 0"));
        assert!(!keyboard_state_is_neutral("0 1 0 0 0"));
        assert!(!keyboard_state_is_neutral("0 0 2 0 0"));
        assert!(!keyboard_state_is_neutral("0 0 0 1 0"));
        assert!(!keyboard_state_is_neutral("0 0 0 0 1"));
        assert!(!keyboard_state_is_neutral("invalid"));
    }
}
