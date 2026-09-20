pub mod click;
pub mod drag;
pub mod idle;
pub mod mouse;
pub mod scroll;
pub mod style;
pub mod typing;

use serde::{Deserialize, Serialize};

pub use style::{HumanizerStyle, SpeedClass, draw_style};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanizerConfig {
    pub seed: u64,
    pub style: HumanizerStyle,
}

impl HumanizerConfig {
    pub fn from_seed(seed: u64) -> Self {
        Self {
            seed,
            style: draw_style(seed),
        }
    }

    pub fn with_style(seed: u64, style: HumanizerStyle) -> Self {
        Self { seed, style }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedEvent {
    pub timestamp_us: u64,
    pub event: InputEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    MouseRel { dx: i32, dy: i32 },
    MouseButton { button: u8, pressed: bool },
    Key { code: u16, pressed: bool },
    Wheel { delta: i32, hi_res_delta: i32 },
    Sync,
}

#[derive(Debug, Clone, Default)]
pub struct ActionPlan {
    pub events: Vec<TimedEvent>,
}

impl ActionPlan {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub fn push(&mut self, timestamp_us: u64, event: InputEvent) {
        self.events.push(TimedEvent { timestamp_us, event });
    }

    pub fn extend(&mut self, other: ActionPlan) {
        self.events.extend(other.events);
    }

    pub fn duration_us(&self) -> u64 {
        self.events.last().map(|e| e.timestamp_us).unwrap_or(0)
    }

    pub fn offset(&mut self, offset_us: u64) {
        for ev in &mut self.events {
            ev.timestamp_us += offset_us;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

pub fn lognormal_sample(rng: &mut impl rand::Rng, mean: f64, stddev: f64) -> f64 {
    let variance = stddev * stddev;
    let mu = (mean * mean / (mean * mean + variance).sqrt()).ln();
    let sigma = (1.0 + variance / (mean * mean)).ln().sqrt();
    (mu + sigma * standard_normal(rng)).exp()
}

/// N(0, 1) by Box-Muller. (`Standard` for f64 is uniform on [0, 1), not normal.)
pub fn standard_normal(rng: &mut impl rand::Rng) -> f64 {
    let u1: f64 = rng.gen_range(f64::EPSILON..1.0);
    let u2: f64 = rng.gen();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

pub fn ms_to_us(ms: f64) -> u64 {
    (ms * 1000.0).max(0.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_plan_offset() {
        let mut plan = ActionPlan::new();
        plan.push(100, InputEvent::Sync);
        plan.push(200, InputEvent::Sync);
        plan.offset(1000);
        assert_eq!(plan.events[0].timestamp_us, 1100);
        assert_eq!(plan.events[1].timestamp_us, 1200);
    }

    #[test]
    fn lognormal_positive() {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        for _ in 0..100 {
            let v = lognormal_sample(&mut rng, 80.0, 20.0);
            assert!(v > 0.0);
        }
    }

    #[test]
    fn config_from_seed_deterministic() {
        let a = HumanizerConfig::from_seed(99);
        let b = HumanizerConfig::from_seed(99);
        assert_eq!(a.style.fitts_a_ms, b.style.fitts_a_ms);
    }
}
