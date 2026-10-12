use crate::{ActionPlan, HumanizerStyle, InputEvent, lognormal_sample, ms_to_us};
use rand::rngs::StdRng;
use rand::Rng;

pub const BTN_LEFT: u8 = 0;
pub const BTN_RIGHT: u8 = 1;
pub const BTN_MIDDLE: u8 = 2;

/// A click-and-hold with micro-movements during the hold, for checkbox CAPTCHAs and
/// press-and-hold verification buttons. Hold duration is 500-3000ms with tiny cursor drift.
pub fn plan_click_hold(
    hold_ms_min: u64,
    hold_ms_max: u64,
    style: &HumanizerStyle,
    rng: &mut StdRng,
) -> ActionPlan {
    let mut plan = ActionPlan::new();
    let mut t_us: u64 = 0;

    let reaction_ms = lognormal_sample(rng, style.click_reaction_mean_ms, style.click_reaction_stddev_ms);
    t_us += ms_to_us(reaction_ms);

    if rng.gen_bool(0.3) {
        let drift_dx = rng.gen_range(-1..=1);
        let drift_dy = rng.gen_range(-1..=1);
        if drift_dx != 0 || drift_dy != 0 {
            plan.push(t_us, InputEvent::MouseRel { dx: drift_dx, dy: drift_dy });
            plan.push(t_us, InputEvent::Sync);
        }
    }

    plan.push(t_us, InputEvent::MouseButton { button: BTN_LEFT, pressed: true });
    plan.push(t_us, InputEvent::Sync);

    let hold_min = hold_ms_min.max(300) as f64;
    let hold_max = hold_ms_max.max(hold_min as u64 + 100) as f64;
    let hold_mean = (hold_min + hold_max) / 2.0;
    let hold_stddev = (hold_max - hold_min) / 4.0;
    let hold_ms = lognormal_sample(rng, hold_mean, hold_stddev).clamp(hold_min, hold_max * 1.2);

    let micro_move_count = rng.gen_range(2..=6);
    let interval_ms = hold_ms / (micro_move_count as f64 + 1.0);

    for _ in 0..micro_move_count {
        let jitter_ms = lognormal_sample(rng, interval_ms, interval_ms * 0.2);
        t_us += ms_to_us(jitter_ms);
        let dx = rng.gen_range(-1..=1);
        let dy = rng.gen_range(-1..=1);
        if dx != 0 || dy != 0 {
            plan.push(t_us, InputEvent::MouseRel { dx, dy });
            plan.push(t_us, InputEvent::Sync);
        }
    }

    let remaining_ms = hold_ms - (micro_move_count as f64 * interval_ms);
    if remaining_ms > 0.0 {
        t_us += ms_to_us(remaining_ms);
    }

    plan.push(t_us, InputEvent::MouseButton { button: BTN_LEFT, pressed: false });
    plan.push(t_us, InputEvent::Sync);

    plan
}

pub fn plan_click(
    button: u8,
    count: u32,
    style: &HumanizerStyle,
    rng: &mut StdRng,
) -> ActionPlan {
    let mut plan = ActionPlan::new();
    let mut t_us: u64 = 0;

    let reaction_ms = lognormal_sample(rng, style.click_reaction_mean_ms, style.click_reaction_stddev_ms);
    t_us += ms_to_us(reaction_ms);

    for i in 0..count {
        if i > 0 {
            let interval_ms = lognormal_sample(rng, 100.0, 15.0);
            t_us += ms_to_us(interval_ms);
        }

        if rng.gen_bool(0.3) {
            let drift_dx = rng.gen_range(-1..=1);
            let drift_dy = rng.gen_range(-1..=1);
            if drift_dx != 0 || drift_dy != 0 {
                plan.push(t_us, InputEvent::MouseRel { dx: drift_dx, dy: drift_dy });
                plan.push(t_us, InputEvent::Sync);
            }
        }

        plan.push(t_us, InputEvent::MouseButton { button, pressed: true });
        plan.push(t_us, InputEvent::Sync);

        let hold_ms = lognormal_sample(rng, style.click_hold_mean_ms, style.click_hold_stddev_ms);
        t_us += ms_to_us(hold_ms);

        plan.push(t_us, InputEvent::MouseButton { button, pressed: false });
        plan.push(t_us, InputEvent::Sync);
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn click_hold_times_positive() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(42);
        let plan = plan_click(BTN_LEFT, 1, &style, &mut rng);

        let presses: Vec<_> = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::MouseButton { pressed: true, .. }))
            .collect();
        let releases: Vec<_> = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::MouseButton { pressed: false, .. }))
            .collect();

        assert_eq!(presses.len(), 1);
        assert_eq!(releases.len(), 1);
        assert!(releases[0].timestamp_us > presses[0].timestamp_us);
    }

    #[test]
    fn double_click_has_two_presses() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(7);
        let plan = plan_click(BTN_LEFT, 2, &style, &mut rng);

        let press_count = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::MouseButton { pressed: true, .. }))
            .count();
        assert_eq!(press_count, 2);
    }

    #[test]
    fn click_hold_duration_within_bounds() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(42);
        let plan = plan_click_hold(800, 2000, &style, &mut rng);

        let presses: Vec<_> = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::MouseButton { pressed: true, .. }))
            .collect();
        let releases: Vec<_> = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::MouseButton { pressed: false, .. }))
            .collect();

        assert_eq!(presses.len(), 1);
        assert_eq!(releases.len(), 1);
        let hold_us = releases[0].timestamp_us - presses[0].timestamp_us;
        assert!(hold_us >= 300_000, "hold too short: {hold_us}us");
        assert!(hold_us <= 5_000_000, "hold too long: {hold_us}us");
    }

    #[test]
    fn click_hold_has_micro_movements() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(99);
        let plan = plan_click_hold(1000, 2500, &style, &mut rng);

        let rel_count = plan.events.iter()
            .filter(|e| matches!(e.event, InputEvent::MouseRel { .. }))
            .count();
        assert!(rel_count >= 1, "expected micro-movements during hold");
    }
}
