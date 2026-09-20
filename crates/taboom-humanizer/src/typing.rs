use crate::{ActionPlan, HumanizerStyle, InputEvent, lognormal_sample, ms_to_us};
use rand::rngs::StdRng;
use rand::Rng;

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

/// Linux evdev codes follow the physical QWERTY rows, not the alphabet.
const LETTER_KEYCODES: [u16; 26] = [
    30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, // a..m
    49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44, // n..z
];

fn char_to_keycode(ch: char) -> Option<u16> {
    match ch {
        'a'..='z' | 'A'..='Z' => Some(LETTER_KEYCODES[(ch.to_ascii_lowercase() as u8 - b'a') as usize]),
        '0' => Some(11),
        '1'..='9' => Some(2 + (ch as u16 - b'1' as u16)),
        ' ' => Some(57),
        '.' => Some(52),
        ',' => Some(51),
        ';' => Some(39),
        '\'' => Some(40),
        '-' => Some(12),
        '=' => Some(13),
        '[' => Some(26),
        ']' => Some(27),
        '\\' => Some(43),
        '/' => Some(53),
        '`' => Some(41),
        '\n' | '\r' => Some(28),
        '\t' => Some(15),
        _ => None,
    }
}

fn needs_shift(ch: char) -> bool {
    ch.is_ascii_uppercase()
        || matches!(
            ch,
            '!' | '@' | '#' | '$' | '%' | '^' | '&' | '*' | '(' | ')'
                | '_' | '+' | '{' | '}' | '|' | ':' | '"' | '<' | '>' | '?' | '~'
        )
}

fn shift_base(ch: char) -> char {
    match ch {
        '!' => '1', '@' => '2', '#' => '3', '$' => '4', '%' => '5',
        '^' => '6', '&' => '7', '*' => '8', '(' => '9', ')' => '0',
        '_' => '-', '+' => '=', '{' => '[', '}' => ']', '|' => '\\',
        ':' => ';', '"' => '\'', '<' => ',', '>' => '.', '?' => '/',
        '~' => '`',
        c if c.is_ascii_uppercase() => c.to_ascii_lowercase(),
        c => c,
    }
}

const KEY_LEFTSHIFT: u16 = 42;
const KEY_BACKSPACE: u16 = 14;

fn neighbor_key(keycode: u16, rng: &mut StdRng) -> u16 {
    let offset = if rng.gen_bool(0.5) { 1i16 } else { -1 };
    let neighbor = keycode as i16 + offset;
    if (2..=53).contains(&neighbor) {
        neighbor as u16
    } else {
        keycode
    }
}

/// True when every char has a key on the US layout `plan_type` types on; others would be skipped.
pub fn can_type(text: &str) -> bool {
    text.chars()
        .all(|ch| char_to_keycode(if needs_shift(ch) { shift_base(ch) } else { ch }).is_some())
}

pub fn plan_type(
    text: &str,
    style: &HumanizerStyle,
    rng: &mut StdRng,
) -> ActionPlan {
    let chars: Vec<char> = text.chars().filter(|&c| char_to_keycode(if needs_shift(c) { shift_base(c) } else { c }).is_some()).collect();
    let mut keys = KeyTimeline::default();

    // press-to-press baseline; a per-call tempo, then a slow drift across the text
    let base_iki = style.typing_mean_flight_ms + style.typing_hold_mean_ms;
    let mut tempo = lognormal_sample(rng, 1.0, 0.15);
    let mut t = lognormal_sample(rng, 120.0, 50.0);
    let mut prev = ' ';
    let mut shift_down = false;
    let mut last_hold = 0.0;
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
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
        if ch.is_ascii_digit() || (!ch.is_ascii_alphanumeric() && ch != ' ') {
            gap *= rng.gen_range(1.2..1.6);
        }
        // rollover: the next key goes down while the previous one is still held
        if prev.is_ascii_lowercase() && ch.is_ascii_lowercase() && prev != ch
            && rng.gen_bool(style.typing_rollover_rate.clamp(0.0, 1.0))
        {
            gap = gap.min(last_hold * rng.gen_range(0.45..0.85));
        }
        if i > 0 {
            t += gap;
        }

        let use_shift = needs_shift(ch);
        if use_shift && !shift_down {
            let at = t;
            t += lognormal_sample(rng, 55.0, 20.0);
            keys.down(KEY_LEFTSHIFT, at);
            shift_down = true;
        } else if !use_shift && shift_down {
            keys.up(KEY_LEFTSHIFT, t - lognormal_sample(rng, 25.0, 10.0).min(gap * 0.5));
            shift_down = false;
        }

        let code = char_to_keycode(if use_shift { shift_base(ch) } else { ch }).unwrap_or(KEY_SPACE);
        let can_typo = style.typo_rate > 0.0 && ch.is_ascii_alphabetic() && rng.gen_bool(style.typo_rate.min(1.0));

        if can_typo {
            let wrong = neighbor_key(code, rng);
            keys.tap(wrong, t, hold(style, rng));
            // sometimes the slip is only noticed a key or two later
            let mut extra = 0;
            if rng.gen_bool(0.3) {
                for &next in chars.iter().skip(i + 1).take(rng.gen_range(1..=2)) {
                    if !next.is_ascii_lowercase() {
                        break;
                    }
                    t += base_iki * tempo * lognormal_sample(rng, 1.0, 0.2);
                    keys.tap(char_to_keycode(next).unwrap_or(KEY_SPACE), t, hold(style, rng));
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
        keys.tap(code, t, last_hold);
        prev = ch;
        i += 1;
    }
    if shift_down {
        let last = keys.last_up();
        keys.up(KEY_LEFTSHIFT, last + lognormal_sample(rng, 30.0, 10.0));
    }
    keys.into_plan()
}

fn hold(style: &HumanizerStyle, rng: &mut StdRng) -> f64 {
    lognormal_sample(rng, style.typing_hold_mean_ms, 18.0).clamp(25.0, 250.0)
}

const KEY_SPACE: u16 = 57;

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

    fn tap(&mut self, code: u16, at: f64, hold_ms: f64) {
        self.down(code, at);
        let pressed = self.events.last().map_or(at, |e| e.0);
        self.up(code, pressed + hold_ms);
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
    #[test]
    fn letters_use_evdev_qwerty_codes() {
        // KEY_Q=16 KEY_A=30 KEY_Z=44 KEY_M=50 KEY_E=18 KEY_H=35 KEY_O=24
        for (ch, code) in [('q', 16), ('a', 30), ('z', 44), ('m', 50), ('e', 18), ('h', 35), ('o', 24), ('P', 25)] {
            assert_eq!(char_to_keycode(ch), Some(code), "{ch}");
        }
    }

    use super::*;
    use rand::SeedableRng;

    /// Replays key events through a US keyboard (shift + keycode -> char, backspace deletes).
    fn typed_text(plan: &ActionPlan) -> String {
        let letters = "abcdefghijklmnopqrstuvwxyz";
        let mut out = String::new();
        let mut shift = false;
        for ev in &plan.events {
            if let InputEvent::Key { code, pressed } = ev.event {
                if code == KEY_LEFTSHIFT {
                    shift = pressed;
                    continue;
                }
                if !pressed {
                    continue;
                }
                if code == KEY_BACKSPACE {
                    out.pop();
                    continue;
                }
                let ch = letters.chars().find(|&c| char_to_keycode(c) == Some(code))
                    .or_else(|| " .,;'-=[]\\/`\n\t0123456789".chars().find(|&c| char_to_keycode(c) == Some(code)))
                    .unwrap_or('?');
                out.push(if shift { match ch { c if c.is_ascii_lowercase() => c.to_ascii_uppercase(), c => SHIFTED.iter().find(|p| p.0 == c).map_or(c, |p| p.1) } } else { ch });
            }
        }
        out
    }

    const SHIFTED: [(char, char); 21] = [
        ('1', '!'), ('2', '@'), ('3', '#'), ('4', '$'), ('5', '%'), ('6', '^'), ('7', '&'),
        ('8', '*'), ('9', '('), ('0', ')'), ('-', '_'), ('=', '+'), ('[', '{'), (']', '}'),
        ('\\', '|'), (';', ':'), ('\'', '"'), (',', '<'), ('.', '>'), ('/', '?'), ('`', '~'),
    ];

    #[test]
    fn typed_result_is_exact_even_with_typos() {
        let mut style = HumanizerStyle::default();
        style.typo_rate = 0.15;
        let text = "Hello World, this is TABOOM typing: 42 keys! user@example.com";
        for seed in 0..100 {
            let mut rng = StdRng::seed_from_u64(seed);
            assert_eq!(typed_text(&plan_type(text, &style, &mut rng)), text, "seed {seed}");
        }
    }

    #[test]
    fn rhythm_varies_and_keys_overlap() {
        let style = HumanizerStyle::default();
        let text = "the quick brown fox jumps over the lazy dog";
        let durations: Vec<u64> = (0..10)
            .map(|seed| plan_type(text, &style, &mut StdRng::seed_from_u64(seed)).duration_us())
            .collect();
        let (min, max) = (durations.iter().min().unwrap(), durations.iter().max().unwrap());
        assert!(*max as f64 > *min as f64 * 1.15, "same speed every time: {durations:?}");

        let plan = plan_type(text, &style, &mut StdRng::seed_from_u64(5));
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
        let mut rng1 = StdRng::seed_from_u64(42);
        let mut rng2 = StdRng::seed_from_u64(42);

        let plan_common = plan_type("the", &style, &mut rng1);
        let plan_rare = plan_type("zxq", &style, &mut rng2);

        assert_ne!(plan_common.duration_us(), plan_rare.duration_us());
    }

    #[test]
    fn typing_produces_key_events() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(7);
        let plan = plan_type("hi", &style, &mut rng);
        let key_presses = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::Key { pressed: true, .. }))
            .count();
        assert!(key_presses >= 2);
    }

    #[test]
    fn shift_used_for_uppercase() {
        let mut style = HumanizerStyle::default();
        style.typo_rate = 0.0;
        let mut rng = StdRng::seed_from_u64(1);
        let plan = plan_type("A", &style, &mut rng);
        let has_shift = plan.events.iter().any(|e| matches!(
            e.event,
            InputEvent::Key { code: 42, pressed: true }
        ));
        assert!(has_shift, "should press shift for uppercase");
    }
}
