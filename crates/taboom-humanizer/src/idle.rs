use crate::{HumanizerStyle, lognormal_sample};
use rand::rngs::StdRng;
use rand::Rng;

/// A resting hand between actions, stepped in real time by the caller. Mostly still, with slow
/// glides that stay near the anchor (where the last action happened). Glides use a minimum-jerk
/// profile with a slight bend, so there is no start/stop snap. Deterministic from the rng.
pub struct IdleMotion {
    offset: (f64, f64),
    glide: Option<Glide>,
    rest_left_ms: f64,
}

struct Glide {
    from: (f64, f64),
    to: (f64, f64),
    bend: f64,
    elapsed_ms: f64,
    dur_ms: f64,
}

/// How far the hand wanders from the anchor, in px.
const IDLE_RADIUS_PX: f64 = 40.0;

impl IdleMotion {
    pub fn new() -> Self {
        Self { offset: (0.0, 0.0), glide: None, rest_left_ms: 600.0 }
    }

    /// Call after every action: the hand is now where the action left it.
    pub fn reset(&mut self, hand_on_keyboard: bool, rng: &mut StdRng) {
        self.offset = (0.0, 0.0);
        self.glide = None;
        self.rest_left_ms = if hand_on_keyboard {
            lognormal_sample(rng, 2500.0, 1000.0)
        } else {
            lognormal_sample(rng, 450.0, 200.0)
        };
    }

    /// Movement in px for the next `dt_ms`. Fractional; the caller accumulates sub-pixel parts.
    pub fn step(&mut self, dt_ms: f64, hand_on_keyboard: bool, style: &HumanizerStyle, rng: &mut StdRng) -> (f64, f64) {
        if self.glide.is_none() {
            self.rest_left_ms -= dt_ms;
            if self.rest_left_ms > 0.0 {
                return (0.0, 0.0);
            }
            self.glide = Some(self.next_glide(hand_on_keyboard, style, rng));
        }
        let g = self.glide.as_mut().unwrap();
        g.elapsed_ms = (g.elapsed_ms + dt_ms).min(g.dur_ms);
        let u = g.elapsed_ms / g.dur_ms;
        // minimum-jerk: zero velocity and acceleration at both ends
        let s = u * u * u * (10.0 - 15.0 * u + 6.0 * u * u);
        let (vx, vy) = (g.to.0 - g.from.0, g.to.1 - g.from.1);
        let lateral = g.bend * (std::f64::consts::PI * s).sin();
        let len = (vx * vx + vy * vy).sqrt().max(1e-9);
        let pos = (
            g.from.0 + vx * s - vy / len * lateral,
            g.from.1 + vy * s + vx / len * lateral,
        );
        let delta = (pos.0 - self.offset.0, pos.1 - self.offset.1);
        self.offset = pos;
        if u >= 1.0 {
            self.glide = None;
            self.rest_left_ms = if hand_on_keyboard {
                lognormal_sample(rng, 3500.0, 1500.0)
            } else {
                lognormal_sample(rng, 650.0, 400.0)
            };
        }
        delta
    }

    fn next_glide(&self, hand_on_keyboard: bool, style: &HumanizerStyle, rng: &mut StdRng) -> Glide {
        let roll: f64 = rng.gen();
        let reach = if hand_on_keyboard {
            rng.gen_range(2.0..8.0)
        } else if roll < 0.6 {
            rng.gen_range(2.0..12.0) * (style.idle_drift_px / 2.0).max(0.5)
        } else if roll < 0.9 {
            rng.gen_range(12.0..30.0)
        } else {
            rng.gen_range(30.0..IDLE_RADIUS_PX * 1.5)
        };
        let angle = rng.gen_range(0.0..std::f64::consts::TAU);
        let mut to = (self.offset.0 + reach * angle.cos(), self.offset.1 + reach * angle.sin());
        // drifting past the radius pulls the hand back toward the anchor
        let r = (to.0 * to.0 + to.1 * to.1).sqrt();
        if r > IDLE_RADIUS_PX {
            let k = IDLE_RADIUS_PX * rng.gen_range(0.3..0.8) / r;
            to = (to.0 * k, to.1 * k);
        }
        let dist = ((to.0 - self.offset.0).powi(2) + (to.1 - self.offset.1).powi(2)).sqrt();
        Glide {
            from: self.offset,
            to,
            bend: rng.gen_range(-0.2..0.2) * dist,
            elapsed_ms: 0.0,
            dur_ms: (250.0 + dist * 18.0) * lognormal_sample(rng, 1.0, 0.25),
        }
    }
}

impl Default for IdleMotion {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn idle_motion_rests_glides_and_stays_near_anchor() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(3);
        let mut idle = IdleMotion::new();
        idle.reset(false, &mut rng);
        let (mut x, mut y) = (0.0_f64, 0.0_f64);
        let (mut still, mut moving, mut max_step) = (0, 0, 0.0_f64);
        for _ in 0..(60_000 / 10) {
            let (dx, dy) = idle.step(10.0, false, &style, &mut rng);
            x += dx;
            y += dy;
            let d = (dx * dx + dy * dy).sqrt();
            max_step = max_step.max(d);
            if d == 0.0 { still += 1 } else { moving += 1 }
            assert!((x * x + y * y).sqrt() <= IDLE_RADIUS_PX + 1.0, "wandered to ({x}, {y})");
        }
        assert!(still > 0 && moving > 0, "should both rest and move");
        assert!(max_step < 3.0, "idle step too fast: {max_step} px per 10 ms");
    }

    #[test]
    fn keyboard_hand_mostly_rests() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(9);
        let mut idle = IdleMotion::new();
        idle.reset(true, &mut rng);
        let travelled: f64 = (0..3000)
            .map(|_| {
                let (dx, dy) = idle.step(10.0, true, &style, &mut rng);
                (dx * dx + dy * dy).sqrt()
            })
            .sum();
        assert!(travelled < 60.0, "hand on keyboard moved {travelled} px in 30 s");
    }

}
