use crate::{ActionPlan, HumanizerStyle, InputEvent, lognormal_sample, ms_to_us};
use rand::rngs::StdRng;
use rand::Rng;

pub fn fitts_duration_ms(distance: f64, target_width: f64, style: &HumanizerStyle) -> f64 {
    let id = (distance / target_width + 1.0).log2();
    style.fitts_a_ms + style.fitts_b_ms * id
}

/// Largest distance, in px, between the aimed point and where the hand lands.
pub const LANDING_MAX_PX: f64 = 3.0;

/// Where a hand lands when aiming at `aim`: people aim at a control, not a pixel, so repeated
/// clicks on one point scatter around it (sd ~1 px, never more than `LANDING_MAX_PX`).
pub fn landing_point(aim: (f64, f64), rng: &mut StdRng) -> (f64, f64) {
    let (dx, dy) = (crate::standard_normal(rng), crate::standard_normal(rng));
    let r = (dx * dx + dy * dy).sqrt();
    let k = if r > LANDING_MAX_PX { LANDING_MAX_PX / r } else { 1.0 };
    (aim.0 + dx * k, aim.1 + dy * k)
}

/// Error function (Abramowitz-Stegun 7.1.26, |error| < 1.5e-7).
fn erf(x: f64) -> f64 {
    let sign = x.signum();
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = t * (0.254_829_592
        + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    sign * (1.0 - poly * (-x * x).exp())
}

/// Progress 0..1 of one submovement at normalized time `u` in 0..1: the lognormal CDF, so
/// speed is an asymmetric bell (fast rise, long tail), scaled to reach exactly 1 at u = 1.
fn lognormal_progress(u: f64, mu: f64, sigma: f64) -> f64 {
    if u <= 0.0 {
        return 0.0;
    }
    if u >= 1.0 {
        return 1.0;
    }
    let cdf = |v: f64| 0.5 * (1.0 + erf((v.ln() - mu) / (sigma * std::f64::consts::SQRT_2)));
    (cdf(u) / cdf(1.0)).min(1.0)
}

struct Stroke {
    start_ms: f64,
    dur_ms: f64,
    dx: f64,
    dy: f64,
    mu: f64,
    sigma: f64,
}

impl Stroke {
    fn progress(&self, t_ms: f64) -> f64 {
        lognormal_progress((t_ms - self.start_ms) / self.dur_ms, self.mu, self.sigma)
    }
}

/// An aimed move as people make it: a primary stroke that lands near the target (often a bit
/// long or short), plus a corrective stroke that starts before the primary has stopped, so the
/// hand never halts in between. Curved, wobbly path; tremor fades out at the landing; polling
/// at ~125 Hz with jitter. Lands exactly on `to`.
pub fn plan_move(
    from: (f64, f64),
    to: (f64, f64),
    target_size: f64,
    style: &HumanizerStyle,
    rng: &mut StdRng,
) -> ActionPlan {
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let distance = (dx * dx + dy * dy).sqrt();

    if distance < 0.5 {
        return ActionPlan::new();
    }

    let (ux, uy) = (dx / distance, dy / distance);
    let (px, py) = (-uy, ux);

    let duration_ms = (fitts_duration_ms(distance, target_size.max(1.0), style)
        * lognormal_sample(rng, 1.0, 0.12))
        .clamp(120.0, 2500.0);

    // Where the primary stroke actually lands relative to the target, in px along/across the path.
    let (along, across) = if distance > 40.0 && rng.gen_bool(style.overshoot_probability.clamp(0.0, 1.0)) {
        let over = (distance * style.overshoot_distance_frac * rng.gen_range(0.4..1.2)).min(45.0);
        (over, rng.gen_range(-0.3..0.3) * over)
    } else if distance > 25.0 && rng.gen_bool(0.35) {
        let short = -(distance * rng.gen_range(0.02..0.07)).min(30.0);
        (short, rng.gen_range(-0.4..0.4) * short.abs())
    } else {
        (0.0, 0.0)
    };
    let aim = (dx + ux * along + px * across, dy + uy * along + py * across);

    let primary = Stroke {
        start_ms: 0.0,
        dur_ms: duration_ms,
        dx: aim.0,
        dy: aim.1,
        mu: rng.gen_range(-0.75..-0.35),
        sigma: rng.gen_range(0.28..0.42),
    };
    let correction_len = (along * along + across * across).sqrt();
    let correction = Stroke {
        start_ms: duration_ms * rng.gen_range(0.72..0.9),
        dur_ms: (90.0 + correction_len * 4.0) * lognormal_sample(rng, 1.0, 0.2),
        dx: dx - aim.0,
        dy: dy - aim.1,
        mu: rng.gen_range(-0.6..-0.3),
        sigma: rng.gen_range(0.3..0.45),
    };
    let end_ms = primary.dur_ms.max(correction.start_ms + correction.dur_ms);

    // Bend: one smooth arc plus a slower wobble, both zero at the ends.
    let bend = rng.gen_range(-0.12..0.12) * distance.min(600.0);
    let wobble_amp = rng.gen_range(0.0..0.025) * distance.min(600.0);
    let wobble_phase = rng.gen_range(0.0..std::f64::consts::TAU);
    let tremor: Vec<(f64, f64, f64)> = (0..3)
        .map(|_| {
            (
                style.tremor_freq_hz * rng.gen_range(0.8..1.25),
                rng.gen_range(0.0..std::f64::consts::TAU),
                rng.gen_range(0.0..std::f64::consts::TAU),
            )
        })
        .collect();

    let position = |t_ms: f64| -> (f64, f64) {
        let s1 = primary.progress(t_ms);
        let s2 = correction.progress(t_ms);
        let arc = (std::f64::consts::PI * s1).sin();
        let lateral = bend * arc + wobble_amp * (2.0 * std::f64::consts::PI * s1 + wobble_phase).sin() * arc;
        // tremor rides on the hand while it moves and dies out as it settles on the target
        let settle = (1.0 - t_ms / end_ms).clamp(0.0, 1.0).sqrt();
        let (mut tx, mut ty) = (0.0, 0.0);
        for &(f, phx, phy) in &tremor {
            let w = std::f64::consts::TAU * f * t_ms / 1000.0;
            tx += (w + phx).sin();
            ty += (w + phy).sin();
        }
        let amp = style.tremor_amplitude_px / 3.0 * settle;
        (
            primary.dx * s1 + correction.dx * s2 + px * lateral + tx * amp,
            primary.dy * s1 + correction.dy * s2 + py * lateral + ty * amp,
        )
    };

    let mut plan = ActionPlan::new();
    let (mut emitted_x, mut emitted_y) = (0.0_f64, 0.0_f64);
    let mut t = rng.gen_range(1.0..8.0);
    while t < end_ms {
        let (x, y) = position(t);
        let (ex, ey) = ((x - emitted_x).round() as i32, (y - emitted_y).round() as i32);
        if ex != 0 || ey != 0 {
            let ts = ms_to_us(t);
            plan.push(ts, InputEvent::MouseRel { dx: ex, dy: ey });
            plan.push(ts, InputEvent::Sync);
            emitted_x += ex as f64;
            emitted_y += ey as f64;
        }
        t += 8.0 * rng.gen_range(0.9..1.1);
    }

    let (fx, fy) = ((dx - emitted_x).round() as i32, (dy - emitted_y).round() as i32);
    if fx != 0 || fy != 0 {
        let ts = ms_to_us(end_ms);
        plan.push(ts, InputEvent::MouseRel { dx: fx, dy: fy });
        plan.push(ts, InputEvent::Sync);
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn replay(plan: &ActionPlan) -> (f64, f64, Vec<(u64, f64)>) {
        let (mut x, mut y) = (0.0, 0.0);
        let mut steps = Vec::new();
        for ev in &plan.events {
            if let InputEvent::MouseRel { dx, dy } = ev.event {
                x += dx as f64;
                y += dy as f64;
                steps.push((ev.timestamp_us, ((dx * dx + dy * dy) as f64).sqrt()));
            }
        }
        (x, y, steps)
    }

    #[test]
    fn lands_exactly_without_jumps_or_stalls() {
        let style = HumanizerStyle::default();
        for seed in 0..200 {
            let mut rng = StdRng::seed_from_u64(seed);
            let plan = plan_move((0.0, 0.0), (700.0, -260.0), 20.0, &style, &mut rng);
            let (x, y, steps) = replay(&plan);
            assert_eq!((x, y), (700.0, -260.0), "seed {seed} landed elsewhere");
            // no teleports: every report is a plausible hand displacement within ~8 ms
            let biggest = steps.iter().map(|s| s.1).fold(0.0, f64::max);
            assert!(biggest < 60.0, "seed {seed}: {biggest} px in one report");
            // one continuous motion: no gap longer than 60 ms between reports mid-move
            let gaps = steps.windows(2).map(|w| w[1].0 - w[0].0).max().unwrap_or(0);
            assert!(gaps < 60_000, "seed {seed}: stalled {gaps} us");
        }
    }

    #[test]
    fn landings_scatter_around_the_aim() {
        let mut rng = StdRng::seed_from_u64(11);
        let pts: Vec<(f64, f64)> = (0..2000).map(|_| landing_point((500.0, 300.0), &mut rng)).collect();
        let exact = pts.iter().filter(|p| (p.0 - 500.0).abs() < 0.5 && (p.1 - 300.0).abs() < 0.5).count();
        assert!(exact < pts.len() / 4, "{exact} of {} landed on the exact pixel", pts.len());
        let far = pts.iter().map(|p| ((p.0 - 500.0).powi(2) + (p.1 - 300.0).powi(2)).sqrt()).fold(0.0, f64::max);
        assert!(far <= LANDING_MAX_PX + 1e-9, "landed {far} px away");
        let mean_x = pts.iter().map(|p| p.0).sum::<f64>() / pts.len() as f64;
        assert!((mean_x - 500.0).abs() < 0.1, "scatter is biased: mean x {mean_x}");
    }

    #[test]
    fn moves_differ_in_shape_and_duration() {
        let style = HumanizerStyle::default();
        let durations: std::collections::HashSet<u64> = (0..20)
            .map(|seed| {
                let mut rng = StdRng::seed_from_u64(seed);
                plan_move((0.0, 0.0), (400.0, 300.0), 20.0, &style, &mut rng).duration_us() / 1000
            })
            .collect();
        assert!(durations.len() > 10, "durations barely vary: {durations:?}");
    }

    #[test]
    fn progress_is_monotonic_and_bounded() {
        let mut last = 0.0;
        for i in 0..=100 {
            let p = lognormal_progress(i as f64 / 100.0, -0.5, 0.35);
            assert!((0.0..=1.0).contains(&p) && p >= last);
            last = p;
        }
        assert_eq!(lognormal_progress(1.0, -0.5, 0.35), 1.0);
    }

    #[test]
    fn fitts_scales_with_distance() {
        let style = HumanizerStyle::default();
        let short = fitts_duration_ms(50.0, 20.0, &style);
        let long = fitts_duration_ms(500.0, 20.0, &style);
        assert!(long > short, "longer distance should take more time");
    }

    #[test]
    fn fitts_scales_with_target_size() {
        let style = HumanizerStyle::default();
        let small = fitts_duration_ms(200.0, 5.0, &style);
        let big = fitts_duration_ms(200.0, 50.0, &style);
        assert!(small > big, "smaller target should take more time");
    }

    #[test]
    fn no_zero_delta_moves() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(42);
        let plan = plan_move((0.0, 0.0), (200.0, 150.0), 20.0, &style, &mut rng);
        for ev in &plan.events {
            if let InputEvent::MouseRel { dx, dy } = &ev.event {
                assert!(
                    *dx != 0 || *dy != 0,
                    "zero-delta mouse move at t={}",
                    ev.timestamp_us
                );
            }
        }
    }

    #[test]
    fn deterministic_output() {
        let style = HumanizerStyle::default();
        let mut rng1 = StdRng::seed_from_u64(7);
        let mut rng2 = StdRng::seed_from_u64(7);
        let a = plan_move((0.0, 0.0), (300.0, 200.0), 15.0, &style, &mut rng1);
        let b = plan_move((0.0, 0.0), (300.0, 200.0), 15.0, &style, &mut rng2);
        assert_eq!(a.events, b.events);
    }

    #[test]
    fn short_move_minimal() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(1);
        let plan = plan_move((100.0, 100.0), (100.2, 100.1), 10.0, &style, &mut rng);
        assert!(plan.is_empty() || plan.events.len() <= 4);
    }
}
