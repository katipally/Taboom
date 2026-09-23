use rand::distributions::{Distribution, Uniform};
use rand::rngs::StdRng;
use rand::Rng;
use serde::{Deserialize, Serialize};
pub use taboom_core::persona::SpeedClass;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanizerStyle {
    pub fitts_a_ms: f64,
    pub fitts_b_ms: f64,
    pub tremor_freq_hz: f64,
    pub tremor_amplitude_px: f64,
    pub overshoot_probability: f64,
    pub overshoot_distance_frac: f64,
    pub click_reaction_mean_ms: f64,
    pub click_reaction_stddev_ms: f64,
    pub click_hold_mean_ms: f64,
    pub click_hold_stddev_ms: f64,
    pub typing_mean_flight_ms: f64,
    pub typing_stddev_flight_ms: f64,
    pub typing_hold_mean_ms: f64,
    pub typing_rollover_rate: f64,
    pub typo_rate: f64,
    pub scroll_burst_min: u32,
    pub scroll_burst_max: u32,
    pub scroll_pause_mean_ms: f64,
    pub idle_drift_interval_ms: f64,
    pub idle_drift_px: f64,
    pub speed: SpeedClass,
}

impl Default for HumanizerStyle {
    fn default() -> Self {
        Self::from_speed(SpeedClass::Medium)
    }
}

impl HumanizerStyle {
    pub fn from_speed(speed: SpeedClass) -> Self {
        let (a, b) = match speed {
            SpeedClass::Slow => (300.0, 180.0),
            SpeedClass::Medium => (200.0, 150.0),
            SpeedClass::Fast => (120.0, 100.0),
        };
        Self {
            fitts_a_ms: a,
            fitts_b_ms: b,
            tremor_freq_hz: 10.0,
            tremor_amplitude_px: 0.5,
            overshoot_probability: 0.15,
            overshoot_distance_frac: 0.08,
            click_reaction_mean_ms: 80.0,
            click_reaction_stddev_ms: 20.0,
            click_hold_mean_ms: 90.0,
            click_hold_stddev_ms: 15.0,
            typing_mean_flight_ms: 80.0,
            typing_stddev_flight_ms: 25.0,
            typing_hold_mean_ms: 70.0,
            typing_rollover_rate: 0.2,
            typo_rate: 0.02,
            scroll_burst_min: 2,
            scroll_burst_max: 5,
            scroll_pause_mean_ms: 300.0,
            idle_drift_interval_ms: 3000.0,
            idle_drift_px: 2.0,
            speed,
        }
    }
}

pub fn draw_style(seed: u64) -> HumanizerStyle {
    use rand::SeedableRng;
    let mut rng = StdRng::seed_from_u64(seed);

    let speed = match rng.gen_range(0u8..3) {
        0 => SpeedClass::Slow,
        1 => SpeedClass::Medium,
        _ => SpeedClass::Fast,
    };

    let mut style = HumanizerStyle::from_speed(speed);

    let jitter = |rng: &mut StdRng, base: f64, pct: f64| -> f64 {
        let lo = base * (1.0 - pct);
        let hi = base * (1.0 + pct);
        Uniform::new(lo, hi).sample(rng)
    };

    style.fitts_a_ms = jitter(&mut rng, style.fitts_a_ms, 0.2);
    style.fitts_b_ms = jitter(&mut rng, style.fitts_b_ms, 0.2);
    style.tremor_freq_hz = jitter(&mut rng, 10.0, 0.2);
    style.tremor_amplitude_px = jitter(&mut rng, 0.5, 0.3);
    style.overshoot_probability = jitter(&mut rng, 0.15, 0.4).clamp(0.0, 0.5);
    style.click_reaction_mean_ms = jitter(&mut rng, 80.0, 0.3);
    style.click_hold_mean_ms = jitter(&mut rng, 90.0, 0.2);
    style.typing_mean_flight_ms = jitter(&mut rng, 80.0, 0.3);
    style.typing_hold_mean_ms = jitter(&mut rng, 70.0, 0.2);
    style.typing_rollover_rate = jitter(&mut rng, 0.2, 0.3).clamp(0.05, 0.4);
    style.typo_rate = jitter(&mut rng, 0.02, 0.5).clamp(0.0, 0.08);
    style.scroll_pause_mean_ms = jitter(&mut rng, 300.0, 0.3);
    style.idle_drift_interval_ms = jitter(&mut rng, 3000.0, 0.3);
    style.idle_drift_px = jitter(&mut rng, 2.0, 0.3);

    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_draw() {
        let a = draw_style(42);
        let b = draw_style(42);
        assert_eq!(a.fitts_a_ms, b.fitts_a_ms);
        assert_eq!(a.speed, b.speed);
    }

    #[test]
    fn different_seeds_differ() {
        let a = draw_style(1);
        let b = draw_style(2);
        assert_ne!(a.fitts_a_ms, b.fitts_a_ms);
    }
}
