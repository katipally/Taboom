//! Human-derived geometry with heuristic timing. The artifact contains normalized shapes,
//! never source timestamps or absolute coordinates. Two participant groups are reserved for eval.

use crate::{ActionPlan, HumanizerStyle, InputEvent, lognormal_sample, ms_to_us};
use rand::{Rng, rngs::StdRng};
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub schema_version: u32,
    pub provenance: serde_json::Value,
    pub paths: Vec<HumanPath>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanPath {
    pub distance_px: f64,
    pub points: Vec<[f64; 2]>,
}

impl HumanPath {
    fn length(&self) -> f64 {
        self.points.windows(2).map(|p|
            (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1])).sum()
    }
}

impl Calibration {
    pub fn parse(json: &str) -> Result<Self, String> {
        let artifact: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if artifact.schema_version != 1 || artifact.paths.is_empty() {
            return Err("unsupported or empty mouse calibration".into());
        }
        for path in &artifact.paths {
            if !path.distance_px.is_finite() || !(50.0..=800.0).contains(&path.distance_px)
                || path.points.len() != 16
                || path.points.first() != Some(&[0.0, 0.0])
                || path.points.last() != Some(&[1.0, 0.0])
                || path.points.iter().any(|[x, y]| !x.is_finite() || !y.is_finite()
                    || !(-0.15..=1.15).contains(x) || y.abs() > 0.5)
            {
                return Err("invalid or unbounded human mouse path".into());
            }
            let length: f64 = path.points.windows(2)
                .map(|p| (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1])).sum();
            if length > 1.201 {
                return Err("human mouse path is too long".into());
            }
        }
        Ok(artifact)
    }
}

pub fn bundled() -> &'static Calibration {
    static CALIBRATION: OnceLock<Calibration> = OnceLock::new();
    CALIBRATION.get_or_init(|| Calibration::parse(include_str!("../data/human_mouse.json"))
        .expect("bundled human mouse calibration must validate"))
}

pub fn status() -> serde_json::Value {
    serde_json::json!({
        "paths": bundled().paths.len(),
        "provenance": bundled().provenance,
        "scope": "mouse geometry at 50-800 px; timing and other inputs remain heuristic",
    })
}

pub(crate) fn plan_move(
    from: (f64, f64), to: (f64, f64), target_size: f64,
    style: &HumanizerStyle, rng: &mut StdRng,
) -> ActionPlan {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let distance = dx.hypot(dy);
    let paths = &bundled().paths;
    // Prefer source movements of comparable length; both sources keep their own timing.
    let eligible: Vec<_> = paths.iter().filter(|p|
        (0.5..=2.0).contains(&(p.distance_px / distance))
        // Integer reports exaggerate small detours; prefer direct shapes for short moves.
        && (distance >= 150.0 || p.length() <= 1.08)).collect();
    let path = if eligible.is_empty() { &paths[rng.gen_range(0..paths.len())] }
        else { eligible[rng.gen_range(0..eligible.len())] };
    let duration = (crate::mouse::fitts_duration_ms(distance, target_size.max(1.0), style)
        * lognormal_sample(rng, 1.0, 0.12)).clamp(120.0, 2500.0);
    let (mu, sigma) = (rng.gen_range(-0.75..-0.35), rng.gen_range(0.28..0.42));
    let mirror = if rng.gen_bool(0.5) { 1.0 } else { -1.0 };
    let phase = rng.gen_range(0.0..std::f64::consts::TAU);
    let mut plan = ActionPlan::new();
    let (mut emitted_x, mut emitted_y) = (0.0, 0.0);
    let mut t = rng.gen_range(1.0..8.0);
    while t < duration {
        // A small linear component prevents quantized tail motion from stalling.
        let progress = 0.9 * crate::mouse::lognormal_progress(t / duration, mu, sigma) + 0.1 * t / duration;
        let index = progress * (path.points.len() - 1) as f64;
        let i = (index.floor() as usize).min(path.points.len() - 2);
        let fraction = (index - i as f64).clamp(0.0, 1.0);
        let [ax, ay] = path.points[i];
        let [bx, by] = path.points[i + 1];
        let x = ax + (bx - ax) * fraction;
        let y = (ay + (by - ay) * fraction) * mirror;
        // Fade in as well as out, and scale down for short gestures. Starting with a
        // nonzero noise offset creates an artificial one-pixel jump at the first report.
        let tremor = style.tremor_amplitude_px * (distance / 300.0).min(1.0)
            * 4.0 * progress * (1.0 - progress)
            * (std::f64::consts::TAU * style.tremor_freq_hz * t / 1000.0 + phase).sin();
        let px = x * dx - y * dy - tremor * dy / distance;
        let py = x * dy + y * dx + tremor * dx / distance;
        let (ex, ey) = ((px - emitted_x).round() as i32, (py - emitted_y).round() as i32);
        if ex != 0 || ey != 0 {
            plan.push(ms_to_us(t), InputEvent::MouseRel { dx: ex, dy: ey });
            plan.push(ms_to_us(t), InputEvent::Sync);
            emitted_x += ex as f64;
            emitted_y += ey as f64;
        }
        t += rng.gen_range(7.2..8.8);
    }
    let (ex, ey) = ((dx - emitted_x).round() as i32, (dy - emitted_y).round() as i32);
    if ex != 0 || ey != 0 {
        plan.push(ms_to_us(duration), InputEvent::MouseRel { dx: ex, dy: ey });
        plan.push(ms_to_us(duration), InputEvent::Sync);
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn bundled_paths_validate_and_exclude_evaluation_participants() {
        assert!(bundled().paths.len() >= 120);
        let provenance = &bundled().provenance;
        assert_eq!(provenance["reserved_evaluation_participant_groups"], serde_json::json!(["p07", "p08"]));
        assert_eq!(provenance["training_participant_groups"], serde_json::json!(["p01", "p02", "p03", "p04", "p05", "p06"]));
        let mut invalid: serde_json::Value = serde_json::from_str(include_str!("../data/human_mouse.json")).unwrap();
        invalid["paths"][0]["points"][1][1] = serde_json::json!(10);
        assert!(Calibration::parse(&invalid.to_string()).is_err());
    }

    #[test]
    fn empirical_and_heuristic_models_produce_different_paths_with_the_same_seed() {
        let mut style = HumanizerStyle::default();
        let empirical = crate::mouse::plan_move((10., 20.), (310., 120.), 24., &style, &mut StdRng::seed_from_u64(8));
        style.mouse_model = crate::style::MouseModel::Heuristic;
        let heuristic = crate::mouse::plan_move((10., 20.), (310., 120.), 24., &style, &mut StdRng::seed_from_u64(8));
        assert_ne!(empirical.events, heuristic.events);
        let endpoint = empirical.events.iter().fold((0, 0), |(x, y), e| match e.event {
            InputEvent::MouseRel { dx, dy } => (x + dx, y + dy), _ => (x, y),
        });
        assert_eq!(endpoint, (300, 100));
        assert!(empirical.events.windows(2).all(|p| p[0].timestamp_us <= p[1].timestamp_us));
    }
}
