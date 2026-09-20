use crate::{ActionPlan, HumanizerStyle, InputEvent, lognormal_sample, mouse, ms_to_us};
use rand::rngs::StdRng;

pub fn plan_drag(
    from: (f64, f64),
    to: (f64, f64),
    style: &HumanizerStyle,
    rng: &mut StdRng,
) -> ActionPlan {
    let mut plan = ActionPlan::new();
    let mut t_us: u64 = 0;

    let hold_before_ms = lognormal_sample(rng, 120.0, 30.0);
    t_us += ms_to_us(hold_before_ms);

    plan.push(t_us, InputEvent::MouseButton { button: 0, pressed: true });
    plan.push(t_us, InputEvent::Sync);

    let pre_move_ms = lognormal_sample(rng, 80.0, 20.0);
    t_us += ms_to_us(pre_move_ms);

    let mut drag_style = style.clone();
    drag_style.fitts_a_ms *= 1.5;
    drag_style.fitts_b_ms *= 1.3;
    drag_style.overshoot_probability *= 0.3;

    let mut move_plan = mouse::plan_move(from, to, 10.0, &drag_style, rng);
    move_plan.offset(t_us);
    let move_dur = move_plan.duration_us();
    plan.extend(move_plan);

    t_us = move_dur + ms_to_us(50.0);

    let settle_ms = lognormal_sample(rng, 100.0, 25.0);
    t_us += ms_to_us(settle_ms);

    plan.push(t_us, InputEvent::MouseButton { button: 0, pressed: false });
    plan.push(t_us, InputEvent::Sync);

    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn drag_has_press_and_release() {
        let style = HumanizerStyle::default();
        let mut rng = StdRng::seed_from_u64(42);
        let plan = plan_drag((100.0, 100.0), (300.0, 200.0), &style, &mut rng);

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
}
