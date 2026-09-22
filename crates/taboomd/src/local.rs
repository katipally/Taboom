use anyhow::{bail, Context, Result};
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicU8, Ordering};
use taboom_humanizer::idle::IdleMotion;
use taboom_humanizer::{click, drag, mouse, scroll, typing, ActionPlan, HumanizerConfig, HumanizerStyle, InputEvent};
use tracing::debug;

/// Drives the container's desktop: screen via grim, input via the `vinput` daemon's one
/// persistent mouse and keyboard, timed by the humanizer. Clones share the device and style.
#[derive(Clone)]
pub struct LocalExecutor {
    env: Vec<(String, String)>,
    input_path: PathBuf,
    input: Arc<Mutex<Option<BufReader<UnixStream>>>>,
    human: Arc<Mutex<(HumanizerConfig, StdRng)>>,
    /// Held by every agent action; the idle hand only moves when it can take it.
    action: Arc<Mutex<()>>,
    idle: Arc<Mutex<IdleHand>>,
    buttons_held: Arc<AtomicU8>,
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
}

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy)]
enum Axis {
    Vertical,
    Horizontal,
}

const BROWSER_START_TIMEOUT: Duration = Duration::from_secs(20);
const BROWSER_CLOSE_TIMEOUT: Duration = Duration::from_secs(10);
const BROWSER_STOP_TIMEOUT: Duration = Duration::from_secs(15);
/// Chrome's Wayland app_id, whatever its --user-data-dir (read from `swaymsg -t get_tree`).
const BROWSER_APP_ID: &str = "google-chrome";

impl LocalExecutor {
    pub fn new() -> Self {
        let env: Vec<(String, String)> = ["WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "SWAYSOCK"]
            .into_iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect();
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        Self {
            env,
            input_path: Path::new(&runtime).join("taboom-input.sock"),
            input: Arc::new(Mutex::new(None)),
            human: Arc::new(Mutex::new((HumanizerConfig::from_seed(0), StdRng::seed_from_u64(0)))),
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

    /// Each persona keeps a stable style (its "habits", derived from its name), while every
    /// session draws fresh randomness so no two sessions move or type alike.
    pub fn set_persona(&self, persona: &str) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        persona.hash(&mut h);
        let seed = h.finish();
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

    /// Sends commands to vinput over one long-lived connection (held keys and buttons survive
    /// between calls; if taboomd dies, vinput releases everything). Reconnects once on failure.
    fn input(&self, lines: &[String]) -> Result<Vec<String>> {
        let mut guard = self.input.lock().unwrap();
        for attempt in 0..2 {
            if guard.is_none() {
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
        let outputs: serde_json::Value = serde_json::from_slice(&out).context("parsing sway outputs")?;
        let rect = outputs
            .as_array()
            .and_then(|a| a.iter().find(|o| o["active"] == true))
            .map(|o| &o["rect"])
            .context("no active sway output")?;
        let dim = |k: &str| rect[k].as_u64().filter(|v| *v > 0).map(|v| v as u32);
        match (dim("width"), dim("height")) {
            (Some(width), Some(height)) => Ok(Screen { width, height }),
            _ => bail!("sway output has no size"),
        }
    }

    /// The screen or a region, scaled by `scale`. `format` is "png" or "ppm"
    /// (Debian's grim is built without JPEG; ppm is the cheap one for diffing).
    pub fn capture(&self, region: Option<Rect>, scale: f64, format: &str) -> Result<Vec<u8>> {
        let mut args = vec!["-c".to_string(), "-s".to_string(), format!("{scale:.4}")];
        if let Some(r) = region {
            args.extend(["-g".into(), format!("{},{} {}x{}", r.x, r.y, r.w, r.h)]);
        }
        args.extend(["-t".into(), format.into()]);
        args.push("-".into());
        self.run("grim", &args)
    }

    /// Tiny cursor-free frame for change detection, so the idle hand does not count as motion.
    pub fn diff_frame(&self) -> Result<Vec<u8>> {
        self.run("grim", &["-s".into(), "0.125".into(), "-t".into(), "ppm".into(), "-".into()])
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
    /// Human-paced move toward (x, y). The hand lands within a few px of the aim, as a person
    /// aiming at a control does; returns where it landed.
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

    /// Real key presses with human timing and occasional corrected typos. The caller pastes
    /// text this layout cannot type (see `can_type`).
    pub fn key_type(&self, text: &str) -> Result<()> {
        if !typing::can_type(text) {
            bail!("text has characters with no key on this layout; use paste mode");
        }
        let plan = self.humanized(|h, rng| typing::plan_type(text, &h.style, rng));
        let played = self.play(&plan, Axis::Vertical);
        if played.is_err() {
            let _ = self.cmd("release".into());
        }
        played
    }

    pub fn can_type(text: &str) -> bool {
        typing::can_type(text)
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
        if Self::can_type(text) {
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

    /// Makes `persona`'s profile the browser's. taboom-browser reads the persona from `active`.
    /// A Chrome running another profile quits and comes back on the new one through super+b.
    /// Returns whether it restarted.
    pub fn use_profile(&self, persona: &str, profile: &Path, active: &Path) -> Result<bool> {
        let running = browser_process();
        let restart = running.as_ref().is_some_and(|(_, dir)| dir != profile);
        if let Some((pid, _)) = running.filter(|_| restart) {
            self.stop_browser(pid)?;
        }
        if std::fs::read_to_string(active).ok().as_deref() != Some(persona) {
            let tmp = active.with_extension("tmp");
            std::fs::create_dir_all(active.parent().context("active persona file has no folder")?)?;
            std::fs::write(&tmp, persona)?;
            std::fs::rename(&tmp, active)?;
        }
        if restart {
            self.focus_browser()?;
        }
        Ok(restart)
    }

    /// Closes the browser's windows like a person quitting it, which saves tabs and history.
    /// SIGTERM is only the fallback (e.g. a "Leave site?" dialog holds the window open): Chrome
    /// treats it as a session end and drops changes from its last few seconds.
    fn stop_browser(&self, pid: i32) -> Result<()> {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        let exited = |timeout: Duration| {
            let deadline = Instant::now() + timeout;
            while browser_process().is_some_and(|(p, _)| p == pid) {
                if Instant::now() > deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            true
        };
        let _ = self.run("swaymsg", &["-q".into(), format!("[pid={pid}] kill")]);
        if exited(BROWSER_CLOSE_TIMEOUT) {
            return Ok(());
        }
        let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
        if exited(BROWSER_STOP_TIMEOUT) {
            return Ok(());
        }
        bail!("the browser (pid {pid}) did not quit within {}s", (BROWSER_CLOSE_TIMEOUT + BROWSER_STOP_TIMEOUT).as_secs())
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

/// pid and --user-data-dir of Chrome's browser process. Its helpers (renderers, gpu, zygotes)
/// share the process name but carry --type=. A zombie has an empty cmdline and is skipped.
fn browser_process() -> Option<(i32, PathBuf)> {
    std::fs::read_dir("/proc").ok()?.flatten().find_map(|e| {
        let pid = e.file_name().to_str()?.parse().ok()?;
        let comm = std::fs::read_to_string(e.path().join("comm")).ok()?;
        let cmdline = std::fs::read(e.path().join("cmdline")).ok()?;
        Some((pid, browser_profile(comm.trim_end(), &cmdline)?))
    })
}

/// The profile of a browser process, or None for anything else. No flag means Chrome's
/// default profile, which never matches a persona's, so it reads as an empty path.
fn browser_profile(comm: &str, cmdline: &[u8]) -> Option<PathBuf> {
    if comm != "chrome" || cmdline.is_empty() {
        return None;
    }
    let args: Vec<_> = cmdline.split(|b| *b == 0).map(String::from_utf8_lossy).collect();
    if args.iter().any(|a| a.starts_with("--type=")) {
        return None;
    }
    let dir = args.iter().find_map(|a| a.strip_prefix("--user-data-dir=").map(PathBuf::from));
    Some(dir.unwrap_or_default())
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn browser_process_is_told_from_helpers() {
        let browser = b"/usr/bin/google-chrome-stable\0--ozone-platform=wayland\0--user-data-dir=/h/profiles/work\0";
        assert_eq!(browser_profile("chrome", browser), Some(PathBuf::from("/h/profiles/work")));
        let renderer = b"/opt/google/chrome/chrome\0--type=renderer\0--user-data-dir=/h/profiles/work\0";
        assert_eq!(browser_profile("chrome", renderer), None);
        assert_eq!(browser_profile("chrome", b"/opt/google/chrome/chrome\0"), Some(PathBuf::new()));
        assert_eq!(browser_profile("chrome", b""), None);
        assert_eq!(browser_profile("bash", browser), None);
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
    fn focused_node_found_in_nested_tree() {
        let tree = serde_json::json!({
            "focused": false,
            "nodes": [{ "focused": false, "nodes": [], "floating_nodes": [
                { "focused": true, "app_id": "google-chrome" }
            ]}]
        });
        assert_eq!(find_focused(&tree).unwrap()["app_id"], "google-chrome");
    }
}
