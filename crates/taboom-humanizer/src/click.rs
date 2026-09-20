use crate::{ActionPlan, HumanizerStyle, InputEvent, lognormal_sample, ms_to_us};
use rand::rngs::StdRng;
use rand::Rng;

pub const BTN_LEFT: u8 = 0;
pub const BTN_RIGHT: u8 = 1;
pub const BTN_MIDDLE: u8 = 2;

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
}
