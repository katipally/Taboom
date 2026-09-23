use crate::{ActionPlan, HumanizerStyle, InputEvent, lognormal_sample, ms_to_us};
use rand::rngs::StdRng;
use rand::Rng;
use std::collections::HashMap;

fn bigram_speed_factor(prev: char, curr: char) -> f64 {
    let common_bigrams = [
        "th", "he", "in", "er", "an", "re", "on", "at", "en", "nd",
        "ti", "es", "or", "te", "of", "ed", "is", "it", "al", "ar",
        "st", "to", "nt", "ng", "se", "ha", "as", "ou", "io", "le",
    ];
    let pair = format!("{}{}", prev.to_ascii_lowercase(), curr.to_ascii_lowercase());
    if common_bigrams.contains(&pair.as_str()) {
        0.7
    } else if prev == ' ' || curr == ' ' {
        1.2
    } else {
        1.0
    }
}

/// One character's key on the active layout: evdev code plus the modifiers that reach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    pub code: u16,
    pub shift: bool,
    pub altgr: bool,
}

/// `char -> key` for the session's XKB layout, built once from vinput's `lookup` replies so
/// typing never assumes a US keyboard.
#[derive(Debug, Clone)]
pub struct Keymap {
    strokes: HashMap<char, Stroke>,
    shift: u16,
    altgr: Option<u16>,
}

/// Physical rows of the main block, by evdev code (digits, top letters, home, bottom). Typos
/// land on a horizontal neighbor in the same row whatever the layout prints on it.
const ROWS: [std::ops::RangeInclusive<u16>; 4] = [2..=13, 16..=27, 30..=40, 44..=53];

/// What `Keymap::build` asks the layout about: printable ASCII and Latin-1, Enter and Tab.
pub fn candidate_chars() -> impl Iterator<Item = char> {
    ('\u{20}'..='\u{7e}').chain('\u{a0}'..='\u{ff}').chain(['\u{20ac}', '\n', '\t'])
}

/// The XKB keysym name vinput resolves for `ch`.
pub fn keysym_name(ch: char) -> String {
    match ch {
        '\n' => "Return".into(),
        '\t' => "Tab".into(),
        ' ' => "space".into(),
        c => format!("U{:04X}", c as u32),
    }
}

impl Keymap {
    /// `lookup(keysym name) -> (evdev code, level)`, level bit 0 = Shift, bit 1 = AltGr.
    /// One lookup per candidate char: O(chars) calls, each O(keycodes x levels) in vinput.
    pub fn build(mut lookup: impl FnMut(&str) -> Option<(u16, u8)>) -> Option<Self> {
        let (shift, _) = lookup("Shift_L")?;
        let altgr = lookup("ISO_Level3_Shift").map(|(code, _)| code);
        let strokes = candidate_chars()
            .filter_map(|ch| {
                let (code, level) = lookup(&keysym_name(ch))?;
                let stroke = Stroke { code, shift: level & 1 != 0, altgr: level & 2 != 0 };
                (!stroke.altgr || altgr.is_some()).then_some((ch, stroke))
            })
            .collect();
        Some(Self { strokes, shift, altgr })
    }

    pub fn stroke(&self, ch: char) -> Option<Stroke> {
        self.strokes.get(&if ch == '\r' { '\n' } else { ch }).copied()
    }

    /// Modifier key codes used by strokes, for exact input paths that must not run the
    /// probabilistic typo planner.
    pub fn modifier_codes(&self) -> (u16, Option<u16>) {
        (self.shift, self.altgr)
    }

    /// A key next to `code` in its physical row that types something on this layout.
    fn neighbor(&self, code: u16, rng: &mut StdRng) -> Option<u16> {
        let row = ROWS.iter().find(|r| r.contains(&code))?;
        let mut sides = [code.wrapping_sub(1), code + 1];
        if rng.gen_bool(0.5) {
            sides.swap(0, 1);
        }
        sides.into_iter().find(|c| row.contains(c) && self.strokes.values().any(|s| s.code == *c))
    }
}

const KEY_BACKSPACE: u16 = 14;

/// True when every char has a key on the session's layout; others would be skipped.
pub fn can_type(text: &str, keymap: &Keymap) -> bool {
    text.chars().all(|ch| keymap.stroke(ch).is_some())
}

pub fn plan_type(
    text: &str,
    keymap: &Keymap,
    style: &HumanizerStyle,
    rng: &mut StdRng,
) -> ActionPlan {
    let chars: Vec<(char, Stroke)> = text.chars().filter_map(|c| Some((c, keymap.stroke(c)?))).collect();
    let mut keys = KeyTimeline::default();

    // press-to-press baseline; a per-call tempo, then a slow drift across the text
    let base_iki = style.typing_mean_flight_ms + style.typing_hold_mean_ms;
    let mut tempo = lognormal_sample(rng, 1.0, 0.15);
    let mut t = lognormal_sample(rng, 120.0, 50.0);
    let mut prev = ' ';
    let mut prev_plain = false;
    let mut held = Modifiers::default();
    let mut last_hold = 0.0;
    let mut i = 0;

    while i < chars.len() {
        let (ch, stroke) = chars[i];
        let plain = !stroke.shift && !stroke.altgr;
        tempo = (0.92 * tempo + 0.08 * lognormal_sample(rng, 1.0, 0.25)).clamp(0.6, 1.8);

        let mut gap = base_iki * bigram_speed_factor(prev, ch) * tempo * lognormal_sample(rng, 1.0, 0.22);
        if prev == ' ' {
            gap += lognormal_sample(rng, 60.0, 30.0);
            if rng.gen_bool(0.035) {
                gap += lognormal_sample(rng, 700.0, 300.0); // choosing the next word
            }
        }
        if matches!(prev, '.' | ',' | '!' | '?' | ';' | ':' | '\n') {
            gap += lognormal_sample(rng, 220.0, 110.0);
        }
        if ch.is_numeric() || (!ch.is_alphanumeric() && ch != ' ') {
            gap *= rng.gen_range(1.2..1.6);
        }
        // rollover: the next key goes down while the previous one is still held
        if prev_plain && plain && prev.is_alphabetic() && ch.is_alphabetic() && prev != ch
            && rng.gen_bool(style.typing_rollover_rate.clamp(0.0, 1.0))
        {
            gap = gap.min(last_hold * rng.gen_range(0.45..0.85));
        }
        if i > 0 {
            t += gap;
        }

        t = held.set(&mut keys, keymap, stroke, t, gap, rng);

        let can_typo = style.typo_rate > 0.0 && ch.is_alphabetic() && rng.gen_bool(style.typo_rate.min(1.0));
        if let Some(wrong) = can_typo.then(|| keymap.neighbor(stroke.code, rng)).flatten() {
            keys.tap(wrong, t, hold(style, rng));
            // sometimes the slip is only noticed a key or two later
            let mut extra = 0;
            if rng.gen_bool(0.3) {
                for &(next, next_stroke) in chars.iter().skip(i + 1).take(rng.gen_range(1..=2)) {
                    if !next.is_alphabetic() || next_stroke.altgr {
                        break;
                    }
                    t += base_iki * tempo * lognormal_sample(rng, 1.0, 0.2);
                    keys.tap(next_stroke.code, t, hold(style, rng));
                    extra += 1;
                }
            }
            t += lognormal_sample(rng, 260.0, 80.0);
            for _ in 0..=extra {
                keys.tap(KEY_BACKSPACE, t, lognormal_sample(rng, 60.0, 12.0));
                t += lognormal_sample(rng, 110.0, 25.0);
            }
        }

        last_hold = hold(style, rng) * if ch == ' ' { 1.15 } else { 1.0 };
        // a repeated key goes down only after its own release; the next modifier change must
        // follow that real press, or "LL" then "o" lifts Shift before the second L
        t = t.max(keys.tap(stroke.code, t, last_hold));
        prev = ch;
        prev_plain = plain;
        i += 1;
    }
    let last = keys.last_up();
    held.release(&mut keys, keymap, last, rng);
    keys.into_plan()
}

/// Shift and AltGr as the hand holds them across keystrokes.
#[derive(Default)]
struct Modifiers {
    shift: bool,
    altgr: bool,
}

impl Modifiers {
    /// Presses or lifts modifiers so `stroke` comes out; returns when its key may go down.
    fn set(&mut self, keys: &mut KeyTimeline, keymap: &Keymap, stroke: Stroke, mut t: f64, gap: f64, rng: &mut StdRng) -> f64 {
        for (want, down, code) in [
            (stroke.shift, &mut self.shift, Some(keymap.shift)),
            (stroke.altgr, &mut self.altgr, keymap.altgr),
        ] {
            let Some(code) = code else { continue };
            if want && !*down {
                keys.down(code, t);
                t += lognormal_sample(rng, 55.0, 20.0);
            } else if !want && *down {
                keys.up(code, t - lognormal_sample(rng, 25.0, 10.0).min(gap * 0.5));
            }
            *down = want;
        }
        t
    }

    fn release(&mut self, keys: &mut KeyTimeline, keymap: &Keymap, last: f64, rng: &mut StdRng) {
        for (down, code) in [(self.shift, Some(keymap.shift)), (self.altgr, keymap.altgr)] {
            if let (true, Some(code)) = (down, code) {
                keys.up(code, last + lognormal_sample(rng, 30.0, 10.0));
            }
        }
        *self = Self::default();
    }
}

fn hold(style: &HumanizerStyle, rng: &mut StdRng) -> f64 {
    lognormal_sample(rng, style.typing_hold_mean_ms, 18.0).clamp(25.0, 250.0)
}


/// Press/release times per key. Presses follow the typing rhythm and releases follow their own
/// hold times, so a fast next key can go down before the previous one is up (rollover). A key
/// is never pressed again before its own release.
#[derive(Default)]
struct KeyTimeline {
    events: Vec<(f64, u16, bool)>,
    released_at: std::collections::HashMap<u16, f64>,
}

impl KeyTimeline {
    fn down(&mut self, code: u16, at: f64) {
        let at = match self.released_at.get(&code) {
            Some(&r) if r.is_finite() => at.max(r + 8.0),
            _ => at,
        };
        self.events.push((at, code, true));
        self.released_at.insert(code, f64::INFINITY);
    }

    fn up(&mut self, code: u16, at: f64) {
        let pressed = self.events.iter().rev().find(|e| e.1 == code && e.2).map_or(0.0, |e| e.0);
        let at = at.max(pressed + 15.0);
        self.events.push((at, code, false));
        self.released_at.insert(code, at);
    }

    /// Returns when the key actually went down.
    fn tap(&mut self, code: u16, at: f64, hold_ms: f64) -> f64 {
        self.down(code, at);
        let pressed = self.events.last().map_or(at, |e| e.0);
        self.up(code, pressed + hold_ms);
        pressed
    }

    fn last_up(&self) -> f64 {
        self.events.iter().filter(|e| !e.2).map(|e| e.0).fold(0.0, f64::max)
    }

    fn into_plan(mut self) -> ActionPlan {
        // releases sort after presses at the same instant, so rollover never drops a key
        self.events.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.2.cmp(&a.2)));
        let mut plan = ActionPlan::new();
        for (t, code, pressed) in self.events {
            let ts = ms_to_us(t);
            plan.push(ts, InputEvent::Key { code, pressed });
            plan.push(ts, InputEvent::Sync);
        }
        plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    const SHIFT: u16 = 42;
    const ALTGR: u16 = 100;

    /// (char, evdev code, level) rows of a layout, as vinput's `lookup` would answer them.
    fn layout(letters: &str, extra: &[(char, u16, u8)]) -> Vec<(char, u16, u8)> {
        const LETTER_CODES: [u16; 26] = [
            30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50,
            49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44,
        ];
        let mut rows = vec![(' ', 57, 0), ('\n', 28, 0), ('\t', 15, 0)];
        for (ch, code) in letters.chars().zip(LETTER_CODES) {
            rows.push((ch, code, 0));
            rows.push((ch.to_ascii_uppercase(), code, 1));
        }
        for (i, ch) in "1234567890".chars().enumerate() {
            rows.push((ch, 2 + i as u16, 0));
        }
        rows.extend_from_slice(extra);
        rows
    }

    fn us() -> Vec<(char, u16, u8)> {
        let mut extra = vec![];
        for (i, ch) in "!@#$%^&*()".chars().enumerate() {
            extra.push((ch, 2 + i as u16, 1));
        }
        for ((plain, shifted), code) in "-=[]\\;',./`".chars().zip("_+{}|:\"<>?~".chars())
            .zip([12, 13, 26, 27, 43, 39, 40, 51, 52, 53, 41])
        {
            extra.push((plain, code, 0));
            extra.push((shifted, code, 1));
        }
        layout("abcdefghijklmnopqrstuvwxyz", &extra)
    }

    /// German QWERTZ: y and z swap, umlauts on the right, @ and € behind AltGr.
    fn de() -> Vec<(char, u16, u8)> {
        let mut extra = vec![
            ('ß', 12, 0), ('ü', 26, 0), ('Ü', 26, 1), ('ö', 39, 0), ('Ö', 39, 1), ('ä', 40, 0), ('Ä', 40, 1),
            (',', 51, 0), (';', 51, 1), ('.', 52, 0), (':', 52, 1), ('-', 53, 0), ('_', 53, 1),
            ('@', 16, 2), ('€', 18, 2),
        ];
        for (i, ch) in "!\"§$%&/()=".chars().enumerate() {
            extra.push((ch, 2 + i as u16, 1));
        }
        layout("abcdefghijklmnopqrstuvwxzy", &extra)
    }

    fn keymap(rows: &[(char, u16, u8)], altgr: bool) -> Keymap {
        Keymap::build(|name| match name {
            "Shift_L" => Some((SHIFT, 0)),
            "ISO_Level3_Shift" => altgr.then_some((ALTGR, 0)),
            _ => rows.iter().find(|r| keysym_name(r.0) == name).map(|r| (r.1, r.2)),
        })
        .unwrap()
    }

    /// Replays key events through the layout: modifiers pick the level, backspace deletes.
    fn typed_text(plan: &ActionPlan, rows: &[(char, u16, u8)]) -> String {
        let mut out = String::new();
        let (mut shift, mut altgr) = (false, false);
        for ev in &plan.events {
            let InputEvent::Key { code, pressed } = ev.event else { continue };
            match code {
                SHIFT => shift = pressed,
                ALTGR => altgr = pressed,
                _ if !pressed => {}
                KEY_BACKSPACE => {
                    out.pop();
                }
                _ => {
                    let level = shift as u8 | (altgr as u8) << 1;
                    out.push(rows.iter().find(|r| r.1 == code && r.2 == level).map_or('?', |r| r.0));
                }
            }
        }
        out
    }

    #[test]
    fn typed_result_is_exact_even_with_typos() {
        let mut style = HumanizerStyle::default();
        style.typo_rate = 0.15;
        for (rows, altgr, text) in [
            (us(), false, "Hello World, this is TABOOM typing: 42 keys! user@example.com"),
            (de(), true, "Zyklus über 5€ für Jürgen, Größe: 42; user@example.de"),
        ] {
            let map = keymap(&rows, altgr);
            assert!(can_type(text, &map), "{text}");
            for seed in 0..100 {
                let plan = plan_type(text, &map, &style, &mut StdRng::seed_from_u64(seed));
                assert_eq!(typed_text(&plan, &rows), text, "seed {seed}");
            }
        }
    }

    #[test]
    fn repeated_shifted_letter_keeps_its_case() {
        let rows = us();
        let map = keymap(&rows, false);
        for (style, text) in [(HumanizerStyle::default(), "OOm"), (HumanizerStyle { typing_hold_mean_ms: 84.0, typing_mean_flight_ms: 56.0, ..Default::default() }, "HELLo")] {
            for seed in 0..5000 {
                let plan = plan_type(text, &map, &style, &mut StdRng::seed_from_u64(seed));
                assert_eq!(typed_text(&plan, &rows), text, "seed {seed}");
            }
        }
    }

    #[test]
    fn layout_decides_the_physical_key() {
        let mut style = HumanizerStyle::default();
        style.typo_rate = 0.0;
        let pressed = |rows: &[(char, u16, u8)], text: &str| -> Vec<u16> {
            plan_type(text, &keymap(rows, true), &style, &mut StdRng::seed_from_u64(3))
                .events
                .iter()
                .filter_map(|e| match e.event {
                    InputEvent::Key { code, pressed: true } => Some(code),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(pressed(&us(), "yz"), [21, 44]);
        assert_eq!(pressed(&de(), "yz"), [44, 21]);
        assert_eq!(pressed(&de(), "@"), [ALTGR, 16]);
        assert_eq!(pressed(&us(), "@"), [SHIFT, 3]);
    }

    #[test]
    fn missing_keys_and_altgr_are_not_typeable() {
        assert!(!can_type("日本", &keymap(&us(), false)));
        assert!(!can_type("ü", &keymap(&us(), false)));
        // AltGr levels are useless when the layout has no ISO_Level3_Shift key
        let no_altgr = keymap(&de(), false);
        assert!(!can_type("@", &no_altgr));
        assert!(can_type("ü\r\n", &no_altgr));
        assert!(Keymap::build(|_| None).is_none());
    }

    #[test]
    fn typos_stay_in_the_physical_row() {
        let mut style = HumanizerStyle::default();
        style.typo_rate = 1.0;
        let map = keymap(&de(), true);
        for seed in 0..50 {
            let plan = plan_type("pqay", &map, &style, &mut StdRng::seed_from_u64(seed));
            for ev in &plan.events {
                if let InputEvent::Key { code, pressed: true } = ev.event {
                    assert!(
                        [KEY_BACKSPACE, 25, 16, 30, 44].contains(&code)
                            || [24, 26, 17, 31, 45].contains(&code),
                        "seed {seed}: key {code} is not a row neighbor"
                    );
                }
            }
        }
    }

    #[test]
    fn rhythm_varies_and_keys_overlap() {
        let style = HumanizerStyle::default();
        let map = keymap(&us(), false);
        let text = "the quick brown fox jumps over the lazy dog";
        let durations: Vec<u64> = (0..10)
            .map(|seed| plan_type(text, &map, &style, &mut StdRng::seed_from_u64(seed)).duration_us())
            .collect();
        let (min, max) = (durations.iter().min().unwrap(), durations.iter().max().unwrap());
        assert!(*max as f64 > *min as f64 * 1.15, "same speed every time: {durations:?}");

        let plan = plan_type(text, &map, &style, &mut StdRng::seed_from_u64(5));
        let mut held = 0i32;
        let mut max_held = 0;
        for ev in &plan.events {
            if let InputEvent::Key { pressed, .. } = ev.event {
                held += if pressed { 1 } else { -1 };
                max_held = max_held.max(held);
            }
        }
        assert_eq!(held, 0, "a key was left down");
        assert!(max_held >= 2, "no rollover at all");
    }

    #[test]
    fn typing_delays_vary_by_bigram() {
        let style = HumanizerStyle::default();
        let map = keymap(&us(), false);
        let plan_common = plan_type("the", &map, &style, &mut StdRng::seed_from_u64(42));
        let plan_rare = plan_type("zxq", &map, &style, &mut StdRng::seed_from_u64(42));
        assert_ne!(plan_common.duration_us(), plan_rare.duration_us());
    }
}
