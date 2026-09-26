//! Export deterministic samples from the shipped humanizer as the same JSONL event stream that
//! vinput writes when VINPUT_TRACE is enabled. Pass a path to write there; without one, stdout is
//! used so CI and shell pipelines can choose their own destination.

use rand::rngs::StdRng;
use rand::SeedableRng;
use serde_json::json;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use taboom_humanizer::click::{self, BTN_LEFT};
use taboom_humanizer::idle::IdleMotion;
use taboom_humanizer::mouse;
use taboom_humanizer::scroll::{self, ScrollDirection};
use taboom_humanizer::typing::{self, Keymap};
use taboom_humanizer::{ActionPlan, HumanizerConfig, HumanizerStyle, InputEvent};

const SEEDS: u64 = 64;
const IDLE_DURATION_MS: u64 = 60_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let mut output: Box<dyn Write> = match args.next() {
        Some(path) => Box::new(BufWriter::new(File::create(path)?)),
        None => Box::new(BufWriter::new(io::stdout().lock())),
    };
    if args.next().is_some() {
        return Err("usage: trace_export [OUTPUT.jsonl]".into());
    }

    for seed in 0..SEEDS {
        let style = HumanizerConfig::from_seed(seed).style;
        let distances = [50.0, 100.0, 200.0, 400.0, 800.0];
        let from = (80.0 + (seed % 13) as f64, 90.0 + (seed % 17) as f64);
        for (variant, distance) in distances.into_iter().enumerate() {
            let mut rng = action_rng(seed, variant as u64);
            let to = (from.0 + distance, from.1);
            let plan = mouse::plan_move(from, to, 20.0, &style, &mut rng);
            let trace_id = format!("{seed}:move:{variant}");
            write_trace(
                &mut output,
                seed,
                "move",
                &trace_id,
                &[(0, "pos", from.0 as i32, from.1 as i32)],
                &plan,
            )?;
        }

        let mut rng = action_rng(seed, 8);
        let plan = click::plan_click(BTN_LEFT, 1, &style, &mut rng);
        write_plan(&mut output, seed, "click", &format!("{seed}:click"), &plan, (100, 100))?;

        let mut rng = action_rng(seed, 9);
        let keymap = Keymap::build(us_lookup).expect("the sample US keymap has a Shift key");
        let plan = typing::plan_type("the quick brown fox jumps over the lazy dog zxq", &keymap, &style, &mut rng);
        write_plan(&mut output, seed, "type", &format!("{seed}:type"), &plan, (100, 100))?;

        let mut rng = action_rng(seed, 10);
        let plan = scroll::plan_scroll(ScrollDirection::Vertical, 20, &style, &mut rng);
        write_plan(&mut output, seed, "scroll", &format!("{seed}:scroll"), &plan, (100, 100))?;

        let mut rng = action_rng(seed, 11);
        write_idle(&mut output, seed, &format!("{seed}:idle"), &style, &mut rng)?;
    }
    output.flush()?;
    Ok(())
}

fn action_rng(seed: u64, action: u64) -> StdRng {
    StdRng::seed_from_u64(seed.wrapping_mul(16).wrapping_add(action))
}

/// JSONL record fields `timestamp_us`, `event`, `a`, and `b` are shared with vinput. Exported
/// records add trace metadata so the lab can group the event stream back into seeded actions.
fn write_event(
    out: &mut impl Write,
    seed: u64,
    action: &str,
    trace_id: &str,
    timestamp_us: u64,
    event: &str,
    a: i32,
    b: i32,
) -> io::Result<()> {
    serde_json::to_writer(
        &mut *out,
        &json!({
            "timestamp_us": timestamp_us,
            "event": event,
            "a": a,
            "b": b,
            "trace_id": trace_id,
            "seed": seed,
            "action": action,
        }),
    )?;
    out.write_all(b"\n")
}

fn write_trace(
    out: &mut impl Write,
    seed: u64,
    action: &str,
    trace_id: &str,
    initial: &[(u64, &str, i32, i32)],
    plan: &ActionPlan,
) -> io::Result<()> {
    for &(t, event, a, b) in initial {
        write_event(out, seed, action, trace_id, t, event, a, b)?;
    }
    let position = initial.last().map(|entry| (entry.2, entry.3)).unwrap_or((0, 0));
    write_plan(out, seed, action, trace_id, plan, position)
}

fn write_plan(
    out: &mut impl Write,
    seed: u64,
    action: &str,
    trace_id: &str,
    plan: &ActionPlan,
    initial_position: (i32, i32),
) -> io::Result<()> {
    let (mut x, mut y) = initial_position;
    for timed in &plan.events {
        match timed.event {
            InputEvent::MouseRel { dx, dy } => {
                x += dx;
                y += dy;
                write_event(out, seed, action, trace_id, timed.timestamp_us, "pos", x, y)?;
            }
            InputEvent::MouseButton { button, pressed } => write_event(
                out,
                seed,
                action,
                trace_id,
                timed.timestamp_us,
                "btn",
                button as i32,
                pressed as i32,
            )?,
            InputEvent::Key { code, pressed } => write_event(
                out,
                seed,
                action,
                trace_id,
                timed.timestamp_us,
                "key",
                code as i32,
                pressed as i32,
            )?,
            InputEvent::Wheel { delta, .. } => {
                // This exporter samples the vertical scroll plan. `delta` is the notch count
                // passed to vinput's `wheel V H` command; hi_res_delta is not an input notch.
                write_event(out, seed, action, trace_id, timed.timestamp_us, "wheel", delta, 0)?;
            }
            InputEvent::Sync => {}
        }
    }
    Ok(())
}

fn write_idle(
    out: &mut impl Write,
    seed: u64,
    trace_id: &str,
    style: &HumanizerStyle,
    rng: &mut StdRng,
) -> io::Result<()> {
    let mut motion = IdleMotion::new();
    motion.reset(false, rng);
    let (mut x, mut y) = (100, 100);
    let (mut remainder_x, mut remainder_y) = (0.0, 0.0);
    write_event(out, seed, "idle", trace_id, 0, "pos", x, y)?;

    for step in 1..=(IDLE_DURATION_MS / 8) {
        let (dx, dy) = motion.step(8.0, false, style, rng);
        remainder_x += dx;
        remainder_y += dy;
        let (ix, iy) = (remainder_x.trunc() as i32, remainder_y.trunc() as i32);
        if ix != 0 || iy != 0 {
            remainder_x -= ix as f64;
            remainder_y -= iy as f64;
            x += ix;
            y += iy;
            write_event(out, seed, "idle", trace_id, step * 8_000, "pos", x, y)?;
        }
    }
    Ok(())
}

/// Minimal US QWERTY mapping for the deterministic English sample string. This is only used to
/// ask the production typing planner for physical key events; it does not implement typing.
fn us_lookup(name: &str) -> Option<(u16, u8)> {
    if name == "Shift_L" {
        return Some((42, 0));
    }
    if name == "ISO_Level3_Shift" {
        return None;
    }
    if name == "Return" {
        return Some((28, 0));
    }
    if name == "Tab" {
        return Some((15, 0));
    }
    if name == "space" {
        return Some((57, 0));
    }
    let scalar = name.strip_prefix('U')?;
    let ch = char::from_u32(u32::from_str_radix(scalar, 16).ok()?)?;
    let (code, shifted) = if ch.is_ascii_alphabetic() {
        let lower = ch.to_ascii_lowercase();
        let code = if let Some(index) = "qwertyuiop".find(lower) {
            16 + index as u16
        } else if let Some(index) = "asdfghjkl".find(lower) {
            30 + index as u16
        } else {
            44 + "zxcvbnm".find(lower)? as u16
        };
        (code, ch.is_ascii_uppercase())
    } else {
        match ch {
            '1'..='9' => (2 + (ch as u16 - '1' as u16), false),
            '0' => (11, false),
            '!' => (2, true),
            '@' => (3, true),
            '#' => (4, true),
            '$' => (5, true),
            '%' => (6, true),
            '^' => (7, true),
            '&' => (8, true),
            '*' => (9, true),
            '(' => (10, true),
            ')' => (11, true),
            '-' => (12, false),
            '_' => (12, true),
            '=' => (13, false),
            '+' => (13, true),
            '[' => (26, false),
            '{' => (26, true),
            ']' => (27, false),
            '}' => (27, true),
            ';' => (39, false),
            ':' => (39, true),
            '\'' => (40, false),
            '"' => (40, true),
            '`' => (41, false),
            '~' => (41, true),
            '\\' => (43, false),
            '|' => (43, true),
            ',' => (51, false),
            '<' => (51, true),
            '.' => (52, false),
            '>' => (52, true),
            '/' => (53, false),
            '?' => (53, true),
            _ => return None,
        }
    };
    Some((code, u8::from(shifted)))
}
